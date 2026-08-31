//! Candidate discovery.
//!
//! The sentinel's whole job, and the first stage of every other profile's.
//! It reads the chain directly rather than trusting the indexer: a keeper that
//! acted on database state alone would act on a view that is, by construction,
//! behind the cluster it is trying to race.
//!
//! Nothing here signs. Discovery is deliberately separable from execution so
//! the wallet-less sentinel can prove the read path works before any profile
//! is given a key.

use std::{error::Error, fmt, time::Duration};

use serde::Deserialize;

/// What a discovery pass observed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub markets: usize,
    pub borrow_positions: usize,
    pub leverage_positions: usize,
    pub slot: u64,
}

#[derive(Debug)]
pub enum DiscoveryError {
    Transport(String),
    Rpc(String),
    Malformed(String),
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(detail) => write!(formatter, "rpc transport failed: {detail}"),
            Self::Rpc(detail) => write!(formatter, "rpc returned an error: {detail}"),
            Self::Malformed(detail) => write!(formatter, "rpc response was malformed: {detail}"),
        }
    }
}

impl Error for DiscoveryError {}

#[derive(Deserialize)]
struct RpcEnvelope<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    message: String,
}

#[derive(Deserialize)]
struct ProgramAccount {
    account: AccountData,
    #[serde(default)]
    pubkey: String,
}

#[derive(Deserialize)]
struct AccountData {
    data: Vec<String>,
}

/// Anchor prefixes every account with an eight-byte discriminator, so counting
/// by type is a prefix match rather than a full decode. The sentinel only
/// needs counts, and decoding every account to get them would cost far more.
pub fn account_discriminator(account_name: &str) -> [u8; 8] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("account:{account_name}").as_bytes());
    let mut discriminator = [0_u8; 8];
    discriminator.copy_from_slice(&digest[..8]);
    discriminator
}

pub struct RpcClient {
    endpoint: String,
    agent: ureq::Agent,
}

impl RpcClient {
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.into(),
            agent: ureq::AgentBuilder::new()
                .timeout_read(timeout)
                .timeout_write(timeout)
                .build(),
        }
    }

    fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, DiscoveryError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let response = self
            .agent
            .post(&self.endpoint)
            .set("content-type", "application/json")
            .send_json(body)
            .map_err(|error| DiscoveryError::Transport(error.to_string()))?;
        let envelope: RpcEnvelope<T> = response
            .into_json()
            .map_err(|error| DiscoveryError::Malformed(error.to_string()))?;
        if let Some(error) = envelope.error {
            return Err(DiscoveryError::Rpc(error.message));
        }
        envelope
            .result
            .ok_or_else(|| DiscoveryError::Malformed("response carried no result".into()))
    }

    pub fn slot(&self) -> Result<u64, DiscoveryError> {
        self.call("getSlot", serde_json::json!([{"commitment": "confirmed"}]))
    }

    /// Count a program's accounts of one type.
    ///
    /// Only the discriminator is fetched: the filter runs on the node, and the
    /// slice keeps a large account set from being pulled over the wire just to
    /// be counted.
    pub fn count_accounts(
        &self,
        program_id: &str,
        discriminator: [u8; 8],
    ) -> Result<usize, DiscoveryError> {
        let encoded = bs58::encode(discriminator).into_string();
        let accounts: Vec<ProgramAccount> = self.call(
            "getProgramAccounts",
            serde_json::json!([
                program_id,
                {
                    "encoding": "base64",
                    "commitment": "confirmed",
                    "dataSlice": { "offset": 0, "length": 0 },
                    "filters": [
                        { "memcmp": { "offset": 0, "bytes": encoded } }
                    ]
                }
            ]),
        )?;
        // The slice makes every payload empty; the count is the answer.
        Ok(accounts.iter().filter(|entry| !entry.account.data.is_empty()).count())
    }

    /// Fetch a program's accounts of one type, with their data.
    ///
    /// The counting variant slices the payload away; this one keeps it,
    /// because acting on a position requires reading it. Kept separate so the
    /// wallet-less sentinel never pays for data it does not use.
    pub fn program_accounts(
        &self,
        program_id: &str,
        discriminator: [u8; 8],
    ) -> Result<Vec<([u8; 32], Vec<u8>)>, DiscoveryError> {
        let encoded = bs58::encode(discriminator).into_string();
        let accounts: Vec<ProgramAccount> = self.call(
            "getProgramAccounts",
            serde_json::json!([
                program_id,
                {
                    "encoding": "base64",
                    "commitment": "confirmed",
                    "filters": [
                        { "memcmp": { "offset": 0, "bytes": encoded } }
                    ]
                }
            ]),
        )?;
        let mut decoded = Vec::with_capacity(accounts.len());
        for entry in accounts {
            let payload = entry
                .account
                .data
                .first()
                .ok_or_else(|| DiscoveryError::Malformed("account carried no data".into()))?;
            let bytes = decode_base64(payload)
                .ok_or_else(|| DiscoveryError::Malformed("account data was not base64".into()))?;
            let address = decode_base58_key(&entry.pubkey)?;
            decoded.push((address, bytes));
        }
        Ok(decoded)
    }

    pub fn account_data(&self, address: &str) -> Result<Vec<u8>, DiscoveryError> {
        #[derive(Deserialize)]
        struct Value {
            value: Option<AccountData>,
        }
        let response: Value = self.call(
            "getAccountInfo",
            serde_json::json!([
                address,
                { "encoding": "base64", "commitment": "confirmed" }
            ]),
        )?;
        let account = response
            .value
            .ok_or_else(|| DiscoveryError::Malformed(format!("{address} does not exist")))?;
        let payload = account
            .data
            .first()
            .ok_or_else(|| DiscoveryError::Malformed("account carried no data".into()))?;
        decode_base64(payload)
            .ok_or_else(|| DiscoveryError::Malformed("account data was not base64".into()))
    }

    pub fn latest_blockhash(&self) -> Result<[u8; 32], DiscoveryError> {
        #[derive(Deserialize)]
        struct Value {
            value: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            blockhash: String,
        }
        let response: Value = self.call(
            "getLatestBlockhash",
            serde_json::json!([{ "commitment": "confirmed" }]),
        )?;
        decode_base58_key(&response.value.blockhash)
    }

    /// Simulate a signed transaction. `Ok(None)` means the program accepted it.
    ///
    /// The signature is not verified by the node here: the transaction is
    /// signed before simulation anyway, and asking the node to verify adds a
    /// failure mode that says nothing about whether the instruction would
    /// succeed.
    pub fn simulate(&self, transaction_base64: &str) -> Result<Option<String>, DiscoveryError> {
        #[derive(Deserialize)]
        struct Value {
            value: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            err: Option<serde_json::Value>,
            logs: Option<Vec<String>>,
        }
        let response: Value = self.call(
            "simulateTransaction",
            serde_json::json!([
                transaction_base64,
                {
                    "commitment": "confirmed",
                    "encoding": "base64",
                    "sigVerify": false,
                    "replaceRecentBlockhash": true
                }
            ]),
        )?;
        let Some(error) = response.value.err else {
            return Ok(None);
        };
        // The logs name the error; the `err` field only gives its number.
        // Both are kept, because classification reads the name and a human
        // reading an alert needs the raw form.
        let named = response
            .value
            .logs
            .unwrap_or_default()
            .into_iter()
            .filter(|line| line.contains("Error") || line.contains("failed"))
            .collect::<Vec<_>>()
            .join(" | ");
        Ok(Some(format!("{error} {named}").trim().to_owned()))
    }

    /// Simulate, and read named accounts as they would stand afterwards.
    ///
    /// Necessary rather than convenient: `fill_liquidation_auction` returns
    /// `Ok` without acting when the auction has already recovered, so a
    /// simulation that succeeds does not mean a fill happened. Only the
    /// post-state shows whether tokens actually moved.
    pub fn simulate_with_accounts(
        &self,
        transaction_base64: &str,
        addresses: &[String],
    ) -> Result<(Option<String>, Vec<Option<Vec<u8>>>), DiscoveryError> {
        #[derive(Deserialize)]
        struct Value {
            value: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            err: Option<serde_json::Value>,
            logs: Option<Vec<String>>,
            accounts: Option<Vec<Option<AccountData>>>,
        }
        let response: Value = self.call(
            "simulateTransaction",
            serde_json::json!([
                transaction_base64,
                {
                    "commitment": "confirmed",
                    "encoding": "base64",
                    "sigVerify": false,
                    "replaceRecentBlockhash": true,
                    "accounts": { "encoding": "base64", "addresses": addresses }
                }
            ]),
        )?;
        let decoded = response
            .value
            .accounts
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                entry
                    .and_then(|account| account.data.first().cloned())
                    .and_then(|payload| decode_base64(&payload))
            })
            .collect();
        let Some(error) = response.value.err else {
            return Ok((None, decoded));
        };
        let named = response
            .value
            .logs
            .unwrap_or_default()
            .into_iter()
            .filter(|line| line.contains("Error") || line.contains("failed"))
            .collect::<Vec<_>>()
            .join(" | ");
        Ok((Some(format!("{error} {named}").trim().to_owned()), decoded))
    }

    pub fn send_transaction(&self, transaction_base64: &str) -> Result<String, DiscoveryError> {
        self.call(
            "sendTransaction",
            serde_json::json!([
                transaction_base64,
                {
                    "encoding": "base64",
                    "preflightCommitment": "confirmed",
                    "maxRetries": 3
                }
            ]),
        )
    }

    pub fn signature_confirmed(&self, signature: &str) -> Result<bool, DiscoveryError> {
        #[derive(Deserialize)]
        struct Value {
            value: Vec<Option<Status>>,
        }
        #[derive(Deserialize)]
        struct Status {
            #[serde(rename = "confirmationStatus")]
            confirmation_status: Option<String>,
            err: Option<serde_json::Value>,
        }
        let response: Value = self.call(
            "getSignatureStatuses",
            serde_json::json!([[signature], { "searchTransactionHistory": true }]),
        )?;
        Ok(response
            .value
            .first()
            .and_then(|entry| entry.as_ref())
            .is_some_and(|status| {
                status.err.is_none()
                    && matches!(
                        status.confirmation_status.as_deref(),
                        Some("confirmed" | "finalized")
                    )
            }))
    }

    pub fn lamports(&self, address: &str) -> Result<u64, DiscoveryError> {
        #[derive(Deserialize)]
        struct Value {
            value: u64,
        }
        let response: Value = self.call(
            "getBalance",
            serde_json::json!([address, { "commitment": "confirmed" }]),
        )?;
        Ok(response.value)
    }
}

fn decode_base58_key(encoded: &str) -> Result<[u8; 32], DiscoveryError> {
    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|error| DiscoveryError::Malformed(error.to_string()))?;
    if bytes.len() != 32 {
        return Err(DiscoveryError::Malformed(format!(
            "expected a 32-byte key, found {} bytes",
            bytes.len()
        )));
    }
    let mut key = [0_u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Standard-alphabet base64 with padding, the encoding the RPC returns.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255_u8; 256];
    for (index, byte) in ALPHABET.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }
    let trimmed = input.trim_end_matches('=');
    let mut output = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u32;
    for byte in trimmed.bytes() {
        let value = lookup[byte as usize];
        if value == 255 {
            return None;
        }
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    Some(output)
}


/// One discovery pass over the pinned programs.
pub fn observe(client: &RpcClient, dusk_program_id: &str) -> Result<Observation, DiscoveryError> {
    let slot = client.slot()?;
    let markets = client.count_accounts(dusk_program_id, account_discriminator("Market"))?;
    let borrow_positions =
        client.count_accounts(dusk_program_id, account_discriminator("BorrowPosition"))?;
    let leverage_positions =
        client.count_accounts(dusk_program_id, account_discriminator("LeveragePosition"))?;
    Ok(Observation {
        borrow_positions,
        leverage_positions,
        markets,
        slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discriminators_are_stable_and_distinct() {
        // Anchor derives these from the account name; a change here means the
        // program's layout changed, not that this code drifted.
        let market = account_discriminator("Market");
        let borrow = account_discriminator("BorrowPosition");
        assert_ne!(market, borrow);
        assert_eq!(market, account_discriminator("Market"));
    }
}
