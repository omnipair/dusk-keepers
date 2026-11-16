//! Explicit wallet spending policy. Reservations are durable before signing.
use super::{
    rpc::RpcClient,
    transaction::{base64, serialize_transaction},
};
use serde::Deserialize;
use std::{collections::BTreeMap, env, fs};

type Result<T> = std::result::Result<T, String>;
const FEES: &str = "SOL_FEES";

/// Maximum gross debit allowed by the instructions, never a simulated estimate.
#[derive(Clone, Debug)]
pub struct TokenDebit {
    pub mint: String,
    pub maximum: u64,
}

/// Inventory settlement uses fixed-gross SPL TransferChecked instructions.
/// The signed instruction fixes the debit even if prices or fees change later.
pub fn inventory_debits(
    instructions: &[super::transaction::Instruction],
    buyer: [u8; 32],
) -> Result<Vec<TokenDebit>> {
    use dusk_adapter::transfer_fees::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
    let mut debits = Vec::new();
    for instruction in instructions {
        let program = crate::liquidation::plan::address(instruction.program_id);
        if ![TOKEN_PROGRAM, TOKEN_2022_PROGRAM].contains(&program.as_str()) {
            continue;
        }
        if instruction.data.first() != Some(&12)
            || instruction.data.len() != 10
            || instruction.accounts.len() != 4
            || instruction.accounts[3].pubkey != buyer
        {
            return Err("inventory contains an unsupported token debit instruction".into());
        }
        debits.push(TokenDebit {
            mint: crate::liquidation::plan::address(instruction.accounts[1].pubkey),
            maximum: u64::from_le_bytes(instruction.data[1..9].try_into().unwrap()),
        });
    }
    if debits.is_empty() {
        return Err("inventory payment instruction missing".into());
    }
    Ok(debits)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    #[serde(deserialize_with = "amount")]
    pub per_transaction: u64,
    #[serde(deserialize_with = "amount")]
    pub per_day: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalletPolicy {
    pub fees: Limits,
    #[serde(deserialize_with = "amount")]
    pub minimum_sol_reserve: u64,
    pub tokens: BTreeMap<String, Limits>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Policy {
    schema_version: u8,
    profiles: BTreeMap<String, WalletPolicy>,
}

fn amount<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<u64, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(serde::de::Error::custom(
            "expected unsigned decimal-string atoms",
        ));
    }
    value.parse().map_err(serde::de::Error::custom)
}

impl WalletPolicy {
    pub fn load() -> Result<Self> {
        let path = env::var("KEEPER_RISK_POLICY")
            .map_err(|_| "KEEPER_RISK_POLICY is required for mainnet signing")?;
        let raw = fs::read_to_string(path).map_err(|_| "unable to read KEEPER_RISK_POLICY")?;
        let policy: Policy =
            serde_json::from_str(&raw).map_err(|_| "invalid KEEPER_RISK_POLICY")?;
        if policy.schema_version != 1 {
            return Err("unsupported risk policy version".into());
        }
        let profile = env::var("KEEPER_PROFILE").map_err(|_| "KEEPER_PROFILE is required")?;
        let wallet = policy
            .profiles
            .get(&profile)
            .ok_or("risk policy does not cover this profile")?
            .clone();
        wallet.validate()?;
        Ok(wallet)
    }

    fn validate(&self) -> Result<()> {
        if self.minimum_sol_reserve == 0 {
            return Err("SOL reserve must be positive".into());
        }
        for limits in std::iter::once(&self.fees).chain(self.tokens.values()) {
            if limits.per_transaction == 0 || limits.per_day < limits.per_transaction {
                return Err(
                    "risk limits must be positive and daily limits must cover a transaction".into(),
                );
            }
        }
        for mint in self.tokens.keys() {
            crate::liquidation::plan::key(mint).map_err(|_| "invalid risk policy mint")?;
        }
        Ok(())
    }

    fn claims(&self, fee: u64, debits: &[TokenDebit]) -> Result<Vec<Claim>> {
        let mut amounts = BTreeMap::from([(FEES.to_owned(), fee)]);
        for debit in debits.iter().filter(|debit| debit.maximum != 0) {
            crate::liquidation::plan::key(&debit.mint)?;
            let value = amounts.entry(debit.mint.clone()).or_default();
            *value = value
                .checked_add(debit.maximum)
                .ok_or("payment bound overflow")?;
        }
        amounts
            .into_iter()
            .map(|(asset, maximum)| {
                let limits = if asset == FEES {
                    &self.fees
                } else {
                    self.tokens
                        .get(&asset)
                        .ok_or_else(|| format!("no spending policy for mint {asset}"))?
                };
                if maximum > limits.per_transaction {
                    return Err(format!(
                        "{asset} maximum debit {maximum} exceeds transaction cap {}",
                        limits.per_transaction
                    ));
                }
                Ok(Claim {
                    asset,
                    maximum,
                    daily_limit: limits.per_day,
                })
            })
            .collect()
    }
}

/// Apply a configured asset cap during route selection as well as submission.
pub fn payment_cap(mint: [u8; 32], configured: u64) -> Result<u64> {
    if !super::mainnet::enabled() && env::var_os("KEEPER_RISK_POLICY").is_none() {
        return Ok(configured);
    }
    let policy = WalletPolicy::load()?;
    Ok(policy
        .tokens
        .get(&crate::liquidation::plan::address(mint))
        .map_or(0, |limits| configured.min(limits.per_transaction)))
}

#[derive(Debug)]
pub struct Claim {
    asset: String,
    maximum: u64,
    daily_limit: u64,
}

pub fn assess(
    rpc: &RpcClient,
    wallet: &str,
    message: &[u8],
    slot: u64,
    debits: &[TokenDebit],
) -> Result<Vec<Claim>> {
    let policy = WalletPolicy::load()?;
    assess_with_policy(&policy, rpc, wallet, message, slot, debits)
}

fn assess_with_policy(
    policy: &WalletPolicy,
    rpc: &RpcClient,
    wallet: &str,
    message: &[u8],
    slot: u64,
    debits: &[TokenDebit],
) -> Result<Vec<Claim>> {
    let fee: serde_json::Value = rpc
        .call(
            "getFeeForMessage",
            serde_json::json!([
                base64(message), {"commitment":"confirmed", "minContextSlot":slot}
            ]),
        )
        .map_err(|e| e.to_string())?;
    if fee["context"]["slot"]
        .as_u64()
        .is_none_or(|observed| observed < slot)
    {
        return Err("stale fee response".into());
    }
    let fee = fee["value"]
        .as_u64()
        .ok_or("fee unavailable or blockhash expired")?;
    let claims = policy.claims(fee, debits)?;
    // Also account for rent/ATA setup before accepting the SOL reserve. Taking
    // the fee off once more is conservative on RPCs whose post-state includes it.
    let simulated: serde_json::Value = rpc
        .call(
            "simulateTransaction",
            serde_json::json!([
                base64(&serialize_transaction(&[[0; 64]], message)),
                {"encoding":"base64", "commitment":"confirmed", "minContextSlot":slot,
                 "sigVerify":false, "replaceRecentBlockhash":false,
                 "accounts":{"encoding":"base64", "addresses":[wallet]}}
            ]),
        )
        .map_err(|e| e.to_string())?;
    if simulated["context"]["slot"]
        .as_u64()
        .is_none_or(|observed| observed < slot)
        || !simulated["value"]["err"].is_null()
    {
        return Err("final wallet reserve simulation rejected or stale".into());
    }
    let remaining = simulated["value"]["accounts"][0]["lamports"]
        .as_u64()
        .ok_or("simulation omitted wallet balance")?;
    if remaining
        .checked_sub(fee)
        .is_none_or(|balance| balance < policy.minimum_sol_reserve)
    {
        return Err("transaction would breach the wallet SOL reserve".into());
    }
    Ok(claims)
}

/// Serialize budget reservations across every process using this wallet. Pending
/// transactions remain charged across midnight; neither restart nor a revision
/// change resets a wallet's allowance. Successful costs conservatively retain
/// their instruction maximum for the day, rather than trusting advisory quotes.
pub fn reserve(
    tx: &mut postgres::Transaction<'_>,
    work: &str,
    wallet: &str,
    claims: &[Claim],
) -> Result<()> {
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        &[&format!("keeper-wallet-budget:{wallet}")],
    )
    .map_err(|_| "wallet budget lock unavailable")?;
    for claim in claims {
        let used: String = tx.query_one(
            "SELECT COALESCE(SUM(b.maximum),0)::text FROM keeper_wallet_budget b JOIN keeper_flash_work w USING(work_key) WHERE b.wallet=$1 AND b.asset=$2 AND w.outcome NOT IN ('abandoned_before_submission','expired_absent') AND (b.asset='SOL_FEES' OR w.outcome <> 'failed') AND (b.created_at >= date_trunc('day',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' OR w.outcome IN ('reserved','submitted_unknown'))",
            &[&wallet, &claim.asset]
        ).map_err(|_| "wallet budget table unavailable; apply database migrations")?.get(0);
        let used: u128 = used.parse().map_err(|_| "invalid stored wallet budget")?;
        if used + u128::from(claim.maximum) > u128::from(claim.daily_limit) {
            return Err(format!("{} daily wallet budget exhausted", claim.asset));
        }
        tx.execute("INSERT INTO keeper_wallet_budget(work_key,wallet,asset,maximum) VALUES($1,$2,$3,$4::text::numeric)", &[&work,&wallet,&claim.asset,&claim.maximum.to_string()]).map_err(|_| "wallet budget reservation failed")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
