//! Execution: turning an observed position into a sent transaction.
//!
//! The central decision here is that **the program decides what is
//! liquidatable, not the keeper**. A keeper that reimplemented the solvency
//! math would be a second implementation of the protocol's most consequential
//! rule, free to drift from the first one with every parameter change. Instead
//! discovery narrows the set cheaply, simulation asks the program, and only a
//! candidate the program itself accepted is ever signed.
//!
//! That makes a rejected simulation the normal case rather than an error: on a
//! healthy market every position is rejected, every pass, and that is the
//! system working.

use std::{error::Error, fmt};

use dusk_adapter::{InstructionContract, KeeperInstructionArguments, encode_keeper_instruction};
use keeper_core::{OutcomeStatus, ReasonCode, lifecycle::ExpectedRace};

use crate::{
    discovery::{DiscoveryError, RpcClient, account_discriminator},
    signer::TransactionSigner,
    transaction::{AccountMeta, Instruction, base64, compile_message, serialize_transaction},
};

/// A borrow position as the keeper needs to see it.
///
/// Decoded by offset rather than through a full Anchor deserializer: the
/// keeper needs five fields out of twenty, and the account is fixed-layout, so
/// a decoder for the rest would be code that can only rot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BorrowPositionRecord {
    pub address: [u8; 32],
    pub owner: [u8; 32],
    pub market: [u8; 32],
    pub position_id: [u8; 32],
    pub base_collateral: u64,
    pub quote_collateral: u64,
    /// `u8::MAX` means no auction is running.
    pub auction_debt_asset: u8,
}

impl BorrowPositionRecord {
    pub const LEN: usize = 266;

    pub fn has_active_auction(&self) -> bool {
        self.auction_debt_asset != u8::MAX
    }

    pub fn has_collateral(&self) -> bool {
        self.base_collateral > 0 || self.quote_collateral > 0
    }

    pub fn decode(address: [u8; 32], data: &[u8]) -> Option<Self> {
        if data.len() < Self::LEN {
            return None;
        }
        if data[..8] != account_discriminator("BorrowPosition") {
            return None;
        }
        let field = |start: usize| -> [u8; 32] {
            let mut value = [0_u8; 32];
            value.copy_from_slice(&data[start..start + 32]);
            value
        };
        let amount = |start: usize| -> u64 {
            u64::from_le_bytes(data[start..start + 8].try_into().unwrap_or([0; 8]))
        };
        Some(Self {
            address,
            auction_debt_asset: data[240],
            base_collateral: amount(104),
            market: field(40),
            owner: field(8),
            position_id: field(72),
            quote_collateral: amount(112),
        })
    }
}

#[derive(Debug)]
pub enum ExecutionError {
    Discovery(DiscoveryError),
    Encoding(String),
    Assembly(String),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery(error) => write!(formatter, "{error}"),
            Self::Encoding(detail) => write!(formatter, "instruction encoding failed: {detail}"),
            Self::Assembly(detail) => write!(formatter, "transaction assembly failed: {detail}"),
        }
    }
}

impl Error for ExecutionError {}

impl From<DiscoveryError> for ExecutionError {
    fn from(error: DiscoveryError) -> Self {
        Self::Discovery(error)
    }
}

/// What one attempt on one candidate came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptReport {
    pub position: String,
    pub debt_mint: String,
    pub status: OutcomeStatus,
    pub reason: ReasonCode,
    pub race: Option<ExpectedRace>,
    pub signature: Option<String>,
    pub detail: Option<String>,
}

impl AttemptReport {
    fn skipped(
        position: &str,
        debt_mint: &str,
        race: ExpectedRace,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            debt_mint: debt_mint.to_owned(),
            detail: Some(detail.into()),
            position: position.to_owned(),
            race: Some(race),
            reason: ReasonCode::BoundsNotMet,
            signature: None,
            status: OutcomeStatus::Skipped,
        }
    }
}

/// Map a program error seen during simulation onto the lifecycle vocabulary.
///
/// The distinction that matters is between "this position is fine" and "this
/// keeper is broken". The first is the overwhelmingly common answer and must
/// stay quiet; the second has to be loud. Anchor error codes are matched as
/// text because that is what simulation returns.
pub fn classify_simulation_failure(detail: &str) -> (ReasonCode, Option<ExpectedRace>) {
    // 6109 is PositionNotLiquidatable, which the program also raises when an
    // auction is already running. Both forms are matched because simulation
    // returns the number in `err` and the name only in the logs.
    if detail.contains("PositionNotLiquidatable") || detail.contains("6109") {
        return (
            ReasonCode::BoundsNotMet,
            Some(ExpectedRace::ObligationNoLongerLiquidatable),
        );
    }
    if detail.contains("InvalidBorrowPosition") || detail.contains("AccountNotInitialized") {
        return (ReasonCode::StateChanged, Some(ExpectedRace::AccountChanged));
    }
    // 6097 is InvalidSettlementPrice, which the program raises when the side
    // being priced carries no collateral. That is a statement about the
    // position, not a fault: this side simply is not a candidate.
    if detail.contains("InvalidSettlementPrice") || detail.contains("6097") {
        return (
            ReasonCode::BoundsNotMet,
            Some(ExpectedRace::ObligationNoLongerLiquidatable),
        );
    }
    if detail.contains("InvalidAssetMint") || detail.contains("InvalidMint") {
        return (ReasonCode::BoundsNotMet, None);
    }
    if detail.contains("BlockhashNotFound") {
        return (ReasonCode::BlockhashExpired, None);
    }
    if detail.contains("insufficient") || detail.contains("InsufficientFunds") {
        return (ReasonCode::InsufficientSignerBalance, None);
    }
    (ReasonCode::SimulationRejected, None)
}

/// The mint pair of one market.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarketMints {
    pub market: [u8; 32],
    pub base: [u8; 32],
    pub quote: [u8; 32],
}

pub struct TriggerJob<'a> {
    pub client: &'a RpcClient,
    pub contract: &'a InstructionContract,
    pub program_id: [u8; 32],
    /// Per market, the pair of mints it trades, used as the candidate debt
    /// assets. A wrong hint cannot cause a wrong action: the program resolves
    /// the mint against the market itself and rejects one that does not
    /// belong. A position whose market is absent is left alone rather than
    /// guessed at.
    pub market_mints: Vec<MarketMints>,
    pub signer: &'a dyn TransactionSigner,
    /// Ceiling on transactions sent per pass, so a bad tick cannot become an
    /// unbounded spend.
    pub max_sends_per_pass: usize,
    pub dry_run: bool,
}

impl TriggerJob<'_> {
    /// Positions worth simulating: initialized, collateralized, and not
    /// already under auction. Everything past this point costs an RPC call, so
    /// the cheap filters run first.
    pub fn candidates(&self) -> Result<Vec<BorrowPositionRecord>, ExecutionError> {
        let accounts = self.client.program_accounts(
            &bs58::encode(self.program_id).into_string(),
            account_discriminator("BorrowPosition"),
        )?;
        Ok(accounts
            .into_iter()
            .filter_map(|(address, data)| BorrowPositionRecord::decode(address, &data))
            .filter(|record| record.has_collateral() && !record.has_active_auction())
            .collect())
    }

    fn trigger_instruction(
        &self,
        position: &BorrowPositionRecord,
        debt_mint: [u8; 32],
    ) -> Result<Instruction, ExecutionError> {
        let data = encode_keeper_instruction(
            self.contract,
            &KeeperInstructionArguments::StartLiquidationAuction,
        )
        .map_err(|error| ExecutionError::Encoding(error.to_string()))?;
        Ok(Instruction {
            accounts: vec![
                AccountMeta::writable(position.market),
                AccountMeta::writable(position.address),
                AccountMeta::readonly(debt_mint),
            ],
            data,
            program_id: self.program_id,
        })
    }

    /// Simulate, and send only what the program accepted.
    pub fn attempt(
        &self,
        position: &BorrowPositionRecord,
        debt_mint: [u8; 32],
    ) -> Result<AttemptReport, ExecutionError> {
        let position_key = bs58::encode(position.address).into_string();
        let mint_key = bs58::encode(debt_mint).into_string();
        let instruction = self.trigger_instruction(position, debt_mint)?;
        let blockhash = self.client.latest_blockhash()?;
        let message = compile_message(self.signer.public_key(), &[instruction], blockhash)
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let signature = self.signer.sign(&message);
        let encoded = base64(&serialize_transaction(&[signature], &message));

        if let Some(detail) = self.client.simulate(&encoded)? {
            let (reason, race) = classify_simulation_failure(&detail);
            return Ok(AttemptReport {
                debt_mint: mint_key,
                detail: Some(detail),
                position: position_key,
                race,
                reason,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        if self.dry_run {
            return Ok(AttemptReport {
                debt_mint: mint_key,
                detail: Some("simulation passed; shadow mode does not send".into()),
                position: position_key,
                race: None,
                reason: ReasonCode::ShadowMode,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        // Re-sign against a fresh blockhash rather than reusing the simulated
        // one: simulation may have taken long enough that the original is
        // close to expiry, and a submitted-then-expired transaction is the
        // one outcome whose result cannot be determined.
        let blockhash = self.client.latest_blockhash()?;
        let message = compile_message(
            self.signer.public_key(),
            &[self.trigger_instruction(position, debt_mint)?],
            blockhash,
        )
        .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let signature = self.signer.sign(&message);
        let encoded = base64(&serialize_transaction(&[signature], &message));

        match self.client.send_transaction(&encoded) {
            Ok(signature) => {
                // A signature is an acknowledgement of receipt, not of
                // inclusion. Reporting it as executed without confirming
                // would make a dropped transaction indistinguishable from a
                // landed one, and the next pass would find the position
                // untouched with no record of why.
                let confirmed = self
                    .client
                    .signature_confirmed(&signature)
                    .unwrap_or(false);
                Ok(AttemptReport {
                    debt_mint: mint_key,
                    detail: if confirmed {
                        None
                    } else {
                        Some("submitted; confirmation not yet observed".into())
                    },
                    position: position_key,
                    race: None,
                    reason: if confirmed {
                        ReasonCode::Confirmed
                    } else {
                        ReasonCode::ConfirmationUnknown
                    },
                    signature: Some(signature),
                    status: OutcomeStatus::Executed,
                })
            }
            Err(error) => {
                let detail = error.to_string();
                let (reason, race) = classify_simulation_failure(&detail);
                Ok(AttemptReport {
                    debt_mint: mint_key,
                    detail: Some(detail),
                    position: position_key,
                    race,
                    reason,
                    signature: None,
                    status: OutcomeStatus::RetryableFailure,
                })
            }
        }
    }

    /// One pass over every candidate.
    pub fn run_pass(&self) -> Result<Vec<AttemptReport>, ExecutionError> {
        let mut reports = Vec::new();
        let mut sent = 0_usize;
        for position in self.candidates()? {
            let position_key = bs58::encode(position.address).into_string();
            let Some(mints) = self
                .market_mints
                .iter()
                .find(|entry| entry.market == position.market)
            else {
                continue;
            };
            // Anchor the declared pair to the chain before using it. The
            // program would reject a wrong mint anyway, but a mismatch here
            // means the deployment's configuration disagrees with the market
            // it names, which is worth saying out loud rather than absorbing
            // as a stream of rejected simulations.
            let declared_base = self
                .client
                .market_base_mint(&bs58::encode(position.market).into_string())?;
            if declared_base != mints.base {
                reports.push(AttemptReport::skipped(
                    &position_key,
                    &bs58::encode(mints.base).into_string(),
                    ExpectedRace::AccountChanged,
                    "configured base mint does not match the market on chain",
                ));
                continue;
            }
            let (base_mint, quote_mint) = (mints.base, mints.quote);
            // Which side carries the debt is not stored on the position, so
            // the program is asked. It is only worth asking about a side whose
            // opposing collateral is nonzero: debt on one side is secured by
            // collateral on the other, and the program rejects a side with no
            // collateral before it ever evaluates health. Skipping those here
            // halves the RPC traffic and keeps a routine non-candidate out of
            // the operational log.
            let mut ordered = Vec::with_capacity(2);
            if position.base_collateral > 0 {
                ordered.push(quote_mint);
            }
            if position.quote_collateral > 0 {
                ordered.push(base_mint);
            }
            for debt_mint in ordered {
                if sent >= self.max_sends_per_pass {
                    reports.push(AttemptReport::skipped(
                        &position_key,
                        &bs58::encode(debt_mint).into_string(),
                        ExpectedRace::LeaseContended,
                        "per-pass send ceiling reached",
                    ));
                    break;
                }
                let report = self.attempt(&position, debt_mint)?;
                let executed = report.status == OutcomeStatus::Executed;
                reports.push(report);
                if executed {
                    sent += 1;
                    // One auction per position; the other side is moot.
                    break;
                }
            }
        }
        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_position(auction_asset: u8, base_collateral: u64) -> Vec<u8> {
        let mut data = vec![0_u8; BorrowPositionRecord::LEN];
        data[..8].copy_from_slice(&account_discriminator("BorrowPosition"));
        data[8..40].copy_from_slice(&[1_u8; 32]);
        data[40..72].copy_from_slice(&[2_u8; 32]);
        data[72..104].copy_from_slice(&[3_u8; 32]);
        data[104..112].copy_from_slice(&base_collateral.to_le_bytes());
        data[240] = auction_asset;
        data
    }

    #[test]
    fn decodes_the_fields_the_keeper_acts_on() {
        let record = BorrowPositionRecord::decode([9; 32], &encoded_position(u8::MAX, 500)).unwrap();
        assert_eq!(record.owner, [1; 32]);
        assert_eq!(record.market, [2; 32]);
        assert_eq!(record.position_id, [3; 32]);
        assert_eq!(record.base_collateral, 500);
        assert!(!record.has_active_auction());
        assert!(record.has_collateral());
    }

    #[test]
    fn an_open_auction_is_visible() {
        let record = BorrowPositionRecord::decode([9; 32], &encoded_position(0, 500)).unwrap();
        assert!(record.has_active_auction());
    }

    #[test]
    fn rejects_an_account_of_another_type() {
        let mut data = encoded_position(u8::MAX, 1);
        data[..8].copy_from_slice(&account_discriminator("Market"));
        assert!(BorrowPositionRecord::decode([9; 32], &data).is_none());
    }

    #[test]
    fn rejects_a_truncated_account() {
        assert!(BorrowPositionRecord::decode([9; 32], &[0_u8; 32]).is_none());
    }

    #[test]
    fn a_healthy_position_is_not_an_error() {
        let (reason, race) = classify_simulation_failure("Error Code: PositionNotLiquidatable");
        assert_eq!(reason, ReasonCode::BoundsNotMet);
        assert_eq!(race, Some(ExpectedRace::ObligationNoLongerLiquidatable));
    }

    #[test]
    fn a_side_without_collateral_is_not_a_candidate() {
        let (reason, race) = classify_simulation_failure("Error Code: InvalidSettlementPrice");
        assert_eq!(reason, ReasonCode::BoundsNotMet);
        assert_eq!(race, Some(ExpectedRace::ObligationNoLongerLiquidatable));
    }

    #[test]
    fn an_unrecognized_failure_stays_loud() {
        let (reason, race) = classify_simulation_failure("Program failed to complete");
        assert_eq!(reason, ReasonCode::SimulationRejected);
        assert_eq!(race, None);
    }
}
