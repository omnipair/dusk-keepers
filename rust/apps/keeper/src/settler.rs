//! Settling a liquidation auction against the backstop.
//!
//! When nobody fills an auction, the price decays to its floor and the
//! position still has to be resolved. The backstop does that: it settles at
//! the floor, drawing on insurance and socializing what insurance cannot
//! cover, and pays whoever called it a bounty for the service.
//!
//! Structurally this is the bidder again — measure what the program would
//! actually pay, then bind the sent transaction to it — but the accounts and
//! the economics differ enough to be their own job. The settler is paid a
//! bounty rather than buying collateral at a discount, and it moves the
//! borrower's debt account rather than its own.

use std::collections::BTreeMap;

use dusk_adapter::{
    AccountLayoutManifest, BackstopLiquidationAuctionArgs, DeterministicAccountResolver,
    InstructionContract, KeeperInstructionArguments, PdaSeedValue, encode_keeper_instruction,
};
use keeper_core::{OutcomeStatus, ReasonCode, lifecycle::ExpectedRace};

use crate::{
    accounts::{AccountAssembler, MarketAccounts, associated_token_account, decode_key},
    bidder::token_amount,
    discovery::RpcClient,
    execute::{AttemptReport, BorrowPositionRecord, ExecutionError, classify_simulation_failure},
    signer::TransactionSigner,
    transaction::{Instruction, base64, compile_message, serialize_transaction},
};

const INSTRUCTION_KEY: &str = "dusk:backstop_liquidation_auction";

pub struct SettlePolicy {
    /// Least collateral the settler will accept as its bounty, in raw atoms.
    /// Below this the call is not worth its fee and is declined.
    pub min_bounty: u64,
    /// How far the sent floor sits below the measured bounty.
    pub slippage_bps: u32,
}

pub struct SettlerJob<'a> {
    pub client: &'a RpcClient,
    pub contract: &'a InstructionContract,
    pub layout: &'a AccountLayoutManifest,
    pub resolver: &'a DeterministicAccountResolver,
    pub signer: &'a dyn TransactionSigner,
    pub policy: SettlePolicy,
    pub dry_run: bool,
}

impl SettlerJob<'_> {
    /// Only positions with an auction running can be settled. Whether the
    /// auction has reached its floor is the program's judgement, not this
    /// one's, so every open auction is offered and refusals are expected.
    pub fn candidates(&self, positions: Vec<BorrowPositionRecord>) -> Vec<BorrowPositionRecord> {
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

    fn settle_instruction(
        &self,
        position: &BorrowPositionRecord,
        market: &MarketAccounts,
        min_caller_bounty_out: u64,
    ) -> Result<(Instruction, [u8; 32]), ExecutionError> {
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

        let caller_collateral_account =
            associated_token_account(signer, token_program, sides.collateral_mint).ok_or_else(
                || ExecutionError::Assembly("caller collateral account does not derive".into()),
            )?;
        // The borrower's own debt account, not the caller's: settlement
        // returns the borrower's surplus to the borrower.
        let owner_debt_account =
            associated_token_account(position.owner, token_program, sides.debt_mint).ok_or_else(
                || ExecutionError::Assembly("owner debt account does not derive".into()),
            )?;

        let mut supplied: BTreeMap<&'static str, [u8; 32]> = BTreeMap::new();
        supplied.insert("market", market.address);
        supplied.insert("borrow_position", position.address);
        supplied.insert("position_owner", position.owner);
        supplied.insert("liquidator", signer);
        supplied.insert("debt_asset_mint", sides.debt_mint);
        supplied.insert("collateral_asset_mint", sides.collateral_mint);
        supplied.insert("debt_reserve_vault", sides.debt_reserve_vault);
        supplied.insert("collateral_reserve_vault", sides.collateral_reserve_vault);
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
        supplied.insert("liquidator_collateral_account", caller_collateral_account);
        supplied.insert("owner_debt_account", owner_debt_account);

        let mut pda_inputs = BTreeMap::new();
        pda_inputs.insert(
            "market".to_owned(),
            PdaSeedValue::Pubkey(bs58::encode(market.address).into_string()),
        );
        pda_inputs.insert(
            "positionId".to_owned(),
            PdaSeedValue::Pubkey(bs58::encode(position.position_id).into_string()),
        );

        let omitted: &[&str] = &["referral_partner", "referral_accrual"];
        let accounts = AccountAssembler {
            contract: self.contract,
            omitted,
            pda_inputs,
            resolver: self.resolver,
            supplied,
        }
        .assemble(INSTRUCTION_KEY)
        .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        // The market update this instruction performs settles hLP, which the
        // program reads from the remaining accounts.
        let accounts = [accounts, market.hlp_remaining_accounts()].concat();

        let data = encode_keeper_instruction(
            self.contract,
            &KeeperInstructionArguments::BackstopLiquidationAuction(
                BackstopLiquidationAuctionArgs {
                    min_caller_bounty_out,
                },
            ),
        )
        .map_err(|error| ExecutionError::Encoding(error.to_string()))?;

        let program_id = decode_key(
            &self
                .contract
                .instructions
                .iter()
                .find(|entry| entry.key == INSTRUCTION_KEY)
                .ok_or_else(|| ExecutionError::Encoding("backstop is not in the contract".into()))?
                .program_id,
        )
        .ok_or_else(|| ExecutionError::Assembly("program id does not decode".into()))?;

        Ok((
            Instruction {
                accounts,
                data,
                program_id,
            },
            caller_collateral_account,
        ))
    }

    fn encode(&self, instruction: Instruction) -> Result<String, ExecutionError> {
        let blockhash = self.client.latest_blockhash()?;
        let message = compile_message(self.signer.public_key(), &[instruction], blockhash)
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let signature = self.signer.sign(&message);
        Ok(base64(&serialize_transaction(&[signature], &message)))
    }

    pub fn attempt(
        &self,
        position: &BorrowPositionRecord,
    ) -> Result<AttemptReport, ExecutionError> {
        let position_key = bs58::encode(position.address).into_string();
        let market = self.market_accounts(position.market)?;

        let (measuring, bounty_account) = self.settle_instruction(position, &market, 0)?;
        let bounty_key = bs58::encode(bounty_account).into_string();
        let before = self
            .client
            .account_data(&bounty_key)
            .ok()
            .as_deref()
            .and_then(token_amount)
            .unwrap_or(0);

        let encoded = self.encode(measuring)?;
        let (failure, accounts) = self
            .client
            .simulate_with_accounts(&encoded, std::slice::from_ref(&bounty_key))?;

        if let Some(detail) = failure {
            let (reason, race) = classify_simulation_failure(&detail);
            return Ok(AttemptReport {
                debt_mint: bounty_key,
                detail: Some(detail),
                position: position_key,
                race,
                reason,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let bounty = accounts
            .first()
            .and_then(Option::as_ref)
            .and_then(|data| token_amount(data))
            .unwrap_or(before)
            .saturating_sub(before);

        // The auction has not reached a point where the backstop pays, or it
        // resolved between passes. Either way there is nothing to do, and the
        // program said so by doing nothing rather than by failing.
        if bounty == 0 {
            return Ok(AttemptReport {
                debt_mint: bounty_key,
                detail: Some("settlement would pay no bounty; nothing would move".into()),
                position: position_key,
                race: Some(ExpectedRace::AuctionAlreadySettled),
                reason: ReasonCode::AlreadyResolved,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        if bounty < self.policy.min_bounty {
            return Ok(AttemptReport {
                debt_mint: bounty_key,
                detail: Some(format!(
                    "bounty of {bounty} is below the {} floor",
                    self.policy.min_bounty
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
                debt_mint: bounty_key,
                detail: Some(format!(
                    "would settle for a bounty of {bounty}; shadow mode does not send"
                )),
                position: position_key,
                race: None,
                reason: ReasonCode::ShadowMode,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let floor = (bounty as u128)
            .saturating_mul((10_000 - self.policy.slippage_bps.min(10_000)) as u128)
            / 10_000;
        let (sending, _) = self.settle_instruction(position, &market, floor as u64)?;
        let encoded = self.encode(sending)?;

        match self.client.send_transaction(&encoded) {
            Ok(signature) => Ok(AttemptReport {
                debt_mint: bounty_key,
                detail: Some(format!("settled for a bounty of at least {floor}")),
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
                    debt_mint: bounty_key,
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
