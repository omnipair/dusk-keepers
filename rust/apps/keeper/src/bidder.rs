//! Filling a liquidation auction.
//!
//! The bidder is the first job that spends rather than merely signs, so what
//! it refuses to do matters more than what it does.
//!
//! It does not reimplement the auction curve. The price decays from a stored
//! start to a stored floor and carries a reservation fee; a keeper that
//! recomputed all that would be a second implementation of the protocol's
//! pricing, free to disagree with the first about how much it is paying. So
//! the fill is **measured**: simulate with no floor, read the collateral that
//! actually arrives, and only then decide.
//!
//! That is also the only way to tell a fill from a no-op. When an auction has
//! recovered, `fill_liquidation_auction` returns `Ok` without moving anything.
//! A bidder that trusted the return code would send a transaction, pay a fee,
//! change nothing, and record it as a successful liquidation.
//!
//! The measured amount then becomes the floor on the transaction that is
//! actually sent, so the terms the bidder accepted are the terms it priced.

use std::collections::BTreeMap;

use dusk_adapter::{
    AccountLayoutManifest, DeterministicAccountResolver, FillLiquidationAuctionArgs,
    InstructionContract, KeeperInstructionArguments, PdaSeedValue, encode_keeper_instruction,
};
use keeper_core::{OutcomeStatus, ReasonCode};

use crate::{
    accounts::{AccountAssembler, MarketAccounts, associated_token_account, decode_key},
    discovery::RpcClient,
    execute::{AttemptReport, BorrowPositionRecord, ExecutionError, classify_simulation_failure},
    signer::TransactionSigner,
    transaction::{Instruction, base64, compile_message, serialize_transaction},
};

const INSTRUCTION_KEY: &str = "dusk:fill_liquidation_auction";
/// SPL token account balances live eight bytes in at offset 64.
const TOKEN_AMOUNT_OFFSET: usize = 64;

pub fn token_amount(data: &[u8]) -> Option<u64> {
    data.get(TOKEN_AMOUNT_OFFSET..TOKEN_AMOUNT_OFFSET + 8)
        .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
        .map(u64::from_le_bytes)
}

pub struct BidPolicy {
    /// Most debt to repay in one fill, in raw atoms. A ceiling the operator
    /// sets, not one the market implies.
    pub max_repay: u64,
    /// Collateral the fill must return per unit of debt repaid, in basis
    /// points, after decimal normalization. Below this the bid is declined.
    ///
    /// There is no default that is right for every market: it encodes what the
    /// operator believes the collateral is worth, which this keeper has no
    /// independent source for. It is deliberately a required setting.
    pub min_collateral_bps: u32,
    /// How far the sent floor sits below the measured amount, to survive the
    /// price decaying a step between simulation and inclusion.
    pub slippage_bps: u32,
}

pub struct BidderJob<'a> {
    pub client: &'a RpcClient,
    pub contract: &'a InstructionContract,
    pub layout: &'a AccountLayoutManifest,
    pub resolver: &'a DeterministicAccountResolver,
    pub signer: &'a dyn TransactionSigner,
    pub policy: BidPolicy,
    pub dry_run: bool,
}

impl BidderJob<'_> {
    /// Positions with an auction running are the only candidates; the trigger
    /// profile is what opens them.
    pub fn candidates(
        &self,
        positions: Vec<BorrowPositionRecord>,
    ) -> Vec<BorrowPositionRecord> {
        positions
            .into_iter()
            .filter(BorrowPositionRecord::has_active_auction)
            .collect()
    }

    fn market_accounts(&self, market: [u8; 32]) -> Result<MarketAccounts, ExecutionError> {
        let data = self
            .client
            .account_data(&bs58::encode(market).into_string())?;
        MarketAccounts::decode(self.layout, market, &data).ok_or_else(|| {
            ExecutionError::Assembly(format!(
                "{} is not a market account",
                bs58::encode(market).into_string()
            ))
        })
    }

    fn fill_instruction(
        &self,
        position: &BorrowPositionRecord,
        market: &MarketAccounts,
        repay_amount: u64,
        min_collateral_out: u64,
    ) -> Result<(Instruction, [u8; 32]), ExecutionError> {
        // `auction_debt_asset` is the market side the auction is settling.
        // Zero is base by the program's own enum ordering.
        let debt_is_base = position.auction_debt_asset == 0;
        let sides = market.oriented(debt_is_base);
        let signer = self.signer.public_key();

        let token_program = decode_key(
            self.resolver
                .resolve_static(INSTRUCTION_KEY, "token_program")
                .ok_or_else(|| {
                    ExecutionError::Assembly("token program is not in the manifest".into())
                })?,
        )
        .ok_or_else(|| ExecutionError::Assembly("token program does not decode".into()))?;

        let liquidator_debt_account =
            associated_token_account(signer, token_program, sides.debt_mint).ok_or_else(|| {
                ExecutionError::Assembly("liquidator debt account does not derive".into())
            })?;
        let liquidator_collateral_account =
            associated_token_account(signer, token_program, sides.collateral_mint).ok_or_else(
                || ExecutionError::Assembly("liquidator collateral account does not derive".into()),
            )?;

        let mut supplied: BTreeMap<&'static str, [u8; 32]> = BTreeMap::new();
        supplied.insert("market", market.address);
        supplied.insert("borrow_position", position.address);
        supplied.insert("position_owner", position.owner);
        supplied.insert("liquidator", signer);
        supplied.insert("debt_asset_mint", sides.debt_mint);
        supplied.insert("collateral_asset_mint", sides.collateral_mint);
        supplied.insert("reserve_vault", sides.debt_reserve_vault);
        supplied.insert("interest_vault", sides.debt_interest_vault);
        supplied.insert("collateral_vault", sides.collateral_collateral_vault);
        supplied.insert(
            "insurance_vault",
            if debt_is_base {
                market.base_insurance_vault
            } else {
                market.quote_insurance_vault
            },
        );
        supplied.insert(
            "collateral_insurance_vault",
            sides.collateral_insurance_vault,
        );
        supplied.insert("liquidator_debt_account", liquidator_debt_account);
        supplied.insert(
            "liquidator_collateral_account",
            liquidator_collateral_account,
        );

        let mut pda_inputs = BTreeMap::new();
        pda_inputs.insert(
            "market".to_owned(),
            PdaSeedValue::Pubkey(bs58::encode(market.address).into_string()),
        );
        pda_inputs.insert(
            "positionId".to_owned(),
            PdaSeedValue::Pubkey(bs58::encode(position.position_id).into_string()),
        );

        // A position with no referrer has none of these accounts, and Anchor
        // encodes an absent optional as the program id.
        let omitted: &[&str] = &["referral_partner", "referral_accrual"];

        let assembler = AccountAssembler {
            contract: self.contract,
            omitted,
            pda_inputs,
            resolver: self.resolver,
            supplied,
        };
        let accounts = assembler
            .assemble(INSTRUCTION_KEY)
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;

        let data = encode_keeper_instruction(
            self.contract,
            &KeeperInstructionArguments::FillLiquidationAuction(FillLiquidationAuctionArgs {
                min_collateral_out,
                repay_amount,
            }),
        )
        .map_err(|error| ExecutionError::Encoding(error.to_string()))?;

        let program_id = decode_key(
            &self
                .contract
                .instructions
                .iter()
                .find(|entry| entry.key == INSTRUCTION_KEY)
                .ok_or_else(|| ExecutionError::Encoding("fill is not in the contract".into()))?
                .program_id,
        )
        .ok_or_else(|| ExecutionError::Assembly("program id does not decode".into()))?;

        Ok((
            Instruction {
                accounts,
                data,
                program_id,
            },
            liquidator_collateral_account,
        ))
    }

    fn encode(&self, instruction: Instruction) -> Result<String, ExecutionError> {
        self.encode_with(instruction, self.client.latest_blockhash()?)
    }

    /// Encode against a caller-supplied blockhash.
    ///
    /// Simulation is asked to replace the blockhash anyway, so fetching a
    /// fresh one for each probe of the cap search doubles the round trips to
    /// change nothing. The send path still takes a current one.
    fn encode_with(
        &self,
        instruction: Instruction,
        blockhash: [u8; 32],
    ) -> Result<String, ExecutionError> {
        let message = compile_message(self.signer.public_key(), &[instruction], blockhash)
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let signature = self.signer.sign(&message);
        Ok(base64(&serialize_transaction(&[signature], &message)))
    }

    /// The largest repayment the program will accept for this position.
    ///
    /// Liquidation is partial: the protocol caps how much of a position one
    /// fill may repay, and a bidder that asked for more is refused outright
    /// rather than trimmed. The cap depends on the position's health and the
    /// market's terms, so this searches for it by simulation instead of
    /// recomputing it — the same reason the price is measured rather than
    /// derived. A dozen simulations is cheap next to a wrong cap formula that
    /// silently drifts from the program.
    fn largest_acceptable_repay(
        &self,
        position: &BorrowPositionRecord,
        market: &MarketAccounts,
    ) -> Result<Option<u64>, ExecutionError> {
        let blockhash = self.client.latest_blockhash()?;
        let accepts = |amount: u64| -> Result<bool, ExecutionError> {
            let (instruction, _) = self.fill_instruction(position, market, amount, 0)?;
            let encoded = self.encode_with(instruction, blockhash)?;
            Ok(self.client.simulate(&encoded)?.is_none())
        };

        if accepts(self.policy.max_repay)? {
            return Ok(Some(self.policy.max_repay));
        }
        let mut low = 0_u64;
        let mut high = self.policy.max_repay;
        // Ten halvings resolve the cap to a thousandth of the ceiling, which
        // is far finer than the fee any fill pays.
        for _ in 0..10 {
            let middle = low + (high - low) / 2;
            if middle == low {
                break;
            }
            if accepts(middle)? {
                low = middle;
            } else {
                high = middle;
            }
        }
        Ok((low > 0).then_some(low))
    }

    pub fn attempt(
        &self,
        position: &BorrowPositionRecord,
    ) -> Result<AttemptReport, ExecutionError> {
        let position_key = bs58::encode(position.address).into_string();
        let market = self.market_accounts(position.market)?;

        let Some(repay_amount) = self.largest_acceptable_repay(position, &market)? else {
            return Ok(AttemptReport {
                debt_mint: String::new(),
                detail: Some("no repayment within the ceiling is acceptable".into()),
                position: position_key,
                race: None,
                reason: ReasonCode::BoundsNotMet,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        };

        // Measure: no floor, so the program reports what the fill would
        // actually return rather than refusing terms the bidder guessed at.
        let (measuring, collateral_account) =
            self.fill_instruction(position, &market, repay_amount, 0)?;
        let collateral_key = bs58::encode(collateral_account).into_string();
        let before = self
            .client
            .account_data(&collateral_key)
            .ok()
            .as_deref()
            .and_then(token_amount)
            .unwrap_or(0);

        let encoded = self.encode(measuring)?;
        let (failure, accounts) = self
            .client
            .simulate_with_accounts(&encoded, std::slice::from_ref(&collateral_key))?;

        if let Some(detail) = failure {
            let (reason, race) = classify_simulation_failure(&detail);
            return Ok(AttemptReport {
                debt_mint: collateral_key,
                detail: Some(detail),
                position: position_key,
                race,
                reason,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let after = accounts
            .first()
            .and_then(Option::as_ref)
            .and_then(|data| token_amount(data))
            .unwrap_or(before);
        let collateral_out = after.saturating_sub(before);

        // Nothing moved: the auction recovered between the trigger and now,
        // and the program said so by doing nothing rather than by failing.
        if collateral_out == 0 {
            return Ok(AttemptReport {
                debt_mint: collateral_key,
                detail: Some("auction is no longer fillable; nothing would move".into()),
                position: position_key,
                race: Some(keeper_core::lifecycle::ExpectedRace::AuctionAlreadySettled),
                reason: ReasonCode::AlreadyResolved,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let required = (repay_amount as u128)
            .saturating_mul(self.policy.min_collateral_bps as u128)
            / 10_000;
        if (collateral_out as u128) < required {
            return Ok(AttemptReport {
                debt_mint: collateral_key,
                detail: Some(format!(
                    "{collateral_out} collateral for {repay_amount} debt is below the {} bps floor",
                    self.policy.min_collateral_bps
                )),
                position: position_key,
                race: None,
                reason: ReasonCode::BoundsNotMet,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        if self.dry_run {
            return Ok(AttemptReport {
                debt_mint: collateral_key,
                detail: Some(format!(
                    "would fill for {collateral_out} collateral; shadow mode does not send"
                )),
                position: position_key,
                race: None,
                reason: ReasonCode::ShadowMode,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        // Bind the send to what was measured, less a tolerance for the price
        // decaying a step between here and inclusion.
        let floor = (collateral_out as u128)
            .saturating_mul((10_000 - self.policy.slippage_bps.min(10_000)) as u128)
            / 10_000;
        let (sending, _) =
            self.fill_instruction(position, &market, repay_amount, floor as u64)?;
        let encoded = self.encode(sending)?;

        match self.client.send_transaction(&encoded) {
            Ok(signature) => Ok(AttemptReport {
                debt_mint: collateral_key,
                detail: Some(format!(
                    "repaid {repay_amount} for at least {floor} collateral"
                )),
                position: position_key,
                race: None,
                reason: ReasonCode::Confirmed,
                signature: Some(signature),
                status: OutcomeStatus::Executed,
            }),
            Err(error) => {
                let detail = error.to_string();
                let (reason, race) = classify_simulation_failure(&detail);
                Ok(AttemptReport {
                    debt_mint: collateral_key,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_spl_token_balance() {
        let mut data = vec![0_u8; 165];
        data[TOKEN_AMOUNT_OFFSET..TOKEN_AMOUNT_OFFSET + 8]
            .copy_from_slice(&1_234_567_u64.to_le_bytes());
        assert_eq!(token_amount(&data), Some(1_234_567));
    }

    #[test]
    fn a_short_account_has_no_balance() {
        assert_eq!(token_amount(&[0_u8; 8]), None);
    }
}
