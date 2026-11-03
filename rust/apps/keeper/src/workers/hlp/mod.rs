//! Market-wide hLP flash repair and terminal recovery.
pub(crate) mod plan;
use crate::{
    liquidation::plan::{Snapshot, address, key},
    liquidation::{compute_limit_rejection, post_bytes, required_u64, simulation, state_hash},
    runtime::execution::{AttemptReport, ExecutionError},
    runtime::flash_journal::{Journal, recover_pending},
    runtime::rpc::{RpcClient, account_discriminator},
    runtime::signer::TransactionSigner,
    runtime::transaction::{Instruction, serialize_transaction},
};
use dusk_adapter::{
    AccountLayoutManifest, InstructionContract, liquidation::HlpFlashQuote, transfer_fees::CLOCK,
};
use keeper_core::{OutcomeStatus, ReasonCode};
use plan::HlpContext;
type Result<T> = std::result::Result<T, String>;

pub struct HlpJob<'a> {
    pub client: &'a RpcClient,
    pub layout: &'a AccountLayoutManifest,
    pub contract: &'a InstructionContract,
    pub signer: &'a dyn TransactionSigner,
    pub dry_run: bool,
    pub max_sends: usize,
}
impl HlpJob<'_> {
    pub fn run_pass(&self) -> std::result::Result<Vec<AttemptReport>, ExecutionError> {
        let mut reports =
            recover_pending(self.client, &self.contract.protocol_revision, self.dry_run)?;
        let configured_market = std::env::var("KEEPER_HLP_MARKET").map_err(|_| {
            ExecutionError::Assembly(
                "KEEPER_HLP_MARKET is required for the hLP recovery profile".into(),
            )
        })?;
        let configured_market_key = key(&configured_market).map_err(ExecutionError::Assembly)?;
        let mut markets = self
            .client
            .program_accounts(&self.layout.program_id, account_discriminator("Market"))?;
        markets.retain(|(market, _)| *market == configured_market_key);
        let mut attempted = 0usize;
        for (market, _) in markets {
            for target_asset in [0u8, 1u8] {
                if attempted >= self.max_sends {
                    return Ok(reports);
                }
                match self.attempt_hlp(market, target_asset) {
                    Ok(Some((signature, detail))) => {
                        if !detail.starts_with("eligible but unfilled:") {
                            attempted += 1;
                        }
                        reports.push(AttemptReport {
                            position: format!("{}:{}", address(market), target_asset),
                            debt_mint: String::new(),
                            status: if signature.is_some() {
                                OutcomeStatus::Executed
                            } else {
                                OutcomeStatus::Skipped
                            },
                            reason: if signature.is_some() {
                                ReasonCode::Confirmed
                            } else {
                                ReasonCode::BoundsNotMet
                            },
                            race: None,
                            signature,
                            detail: Some(detail),
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        attempted += 1;
                        reports.push(AttemptReport {
                            position: format!("{}:{}", address(market), target_asset),
                            debt_mint: String::new(),
                            status: OutcomeStatus::RetryableFailure,
                            reason: ReasonCode::SimulationRejected,
                            race: None,
                            signature: None,
                            detail: Some(error),
                        });
                    }
                }
            }
        }
        Ok(reports)
    }

    fn fresh_hlp(&self, market: [u8; 32], target_asset: u8) -> Result<(HlpContext, Snapshot)> {
        let program = key(&self.layout.program_id)?;
        let first =
            Snapshot::fetch(self.client, &[market], 0).map_err(|error| error.to_string())?;
        let decoded = crate::runtime::accounts::MarketAccounts::decode(
            self.layout,
            market,
            first.owned(market, program)?,
        )
        .ok_or("invalid market")?;
        let initial_keys = [market, decoded.base_mint, decoded.quote_mint, key(CLOCK)?];
        let second = Snapshot::fetch(self.client, &initial_keys, first.slot)
            .map_err(|error| error.to_string())?;
        let context = HlpContext::resolve(
            self.layout,
            &second,
            market,
            self.signer.public_key(),
            target_asset,
        )?;
        let mut keys = initial_keys.to_vec();
        keys.extend(context.named.values().copied());
        keys.extend(
            context
                .market
                .hlp_remaining_accounts()
                .iter()
                .map(|meta| meta.pubkey),
        );
        keys.sort();
        keys.dedup();
        keys.retain(|key| *key != context.program && *key != context.named["event_authority"]);
        if let Ok(table) = std::env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
            keys.push(key(&table)?);
            keys.sort();
            keys.dedup();
        }
        let snapshot =
            Snapshot::fetch(self.client, &keys, second.slot).map_err(|error| error.to_string())?;
        let context = HlpContext::resolve(
            self.layout,
            &snapshot,
            market,
            self.signer.public_key(),
            target_asset,
        )?;
        Ok((context, snapshot))
    }

    fn preview_hlp(
        &self,
        context: &HlpContext,
        snapshot: &Snapshot,
    ) -> Result<Option<HlpFlashQuote>> {
        let mut instructions = crate::liquidation::plan::resources()?;
        instructions.push(context.preview(self.contract)?);
        let value =
            simulation::simulate_for(self.client, context.buyer, snapshot, &instructions, &[])?;
        if !value["value"]["err"].is_null() {
            return if expected_hlp_rejection(&value) || compute_limit_rejection(&value) {
                Ok(None)
            } else {
                Err(format!(
                    "unexpected hLP preview rejection: {}",
                    value["value"]
                ))
            };
        }
        crate::liquidation::plan::decode_return(&value["value"]["returnData"], context.program)
            .map(Some)
    }

    fn attempt_hlp(
        &self,
        market: [u8; 32],
        target_asset: u8,
    ) -> Result<Option<(Option<String>, String)>> {
        let (context, snapshot) = self.fresh_hlp(market, target_asset)?;
        let setup = context.setup(&snapshot)?;
        if !setup.is_empty() {
            return self
                .submit_hlp(&context, &snapshot, "prepare_hlp_accounts", setup, false)
                .map(Some);
        }
        let max_insurance_draw = required_u64("KEEPER_HLP_MAX_INSURANCE_DRAW")?;
        let max_socialized_loss = required_u64("KEEPER_HLP_MAX_SOCIALIZED_LOSS")?;
        let max_payment_debit = required_u64("KEEPER_HLP_MAX_PAYMENT_DEBIT")?;
        let min_collateral_credit = required_u64("KEEPER_HLP_MIN_COLLATERAL_CREDIT")?;
        let recovered = try_hlp_recovery(
            || {
                let Some(quote) = self.preview_hlp(&context, &snapshot)? else {
                    return Ok(None);
                };
                let (instructions, gross_payment) = context.flash_plan(
                    self.contract,
                    &snapshot,
                    &quote,
                    max_insurance_draw,
                    max_socialized_loss,
                )?;
                let buyer_payment_balance = snapshot
                    .accounts
                    .get(&address(context.named["buyer_refund_account"]))
                    .and_then(Option::as_ref)
                    .and_then(|account| crate::liquidation::plan::token_amount(&account.data))
                    .unwrap_or(0);
                if gross_payment > max_payment_debit
                    || gross_payment > buyer_payment_balance
                    || quote.collateral_credit < min_collateral_credit
                    || !self.hlp_flash_meets_bounds(
                        &context,
                        &snapshot,
                        &instructions,
                        max_payment_debit,
                        min_collateral_credit,
                    )?
                {
                    return Ok(None);
                }
                Ok(Some((
                    if quote.full {
                        "hlp_flash_full"
                    } else {
                        "hlp_flash"
                    },
                    instructions,
                )))
            },
            || {
                // Terminal recovery has no buyer payment leg. Its simulation
                // independently checks eligibility and the configured loss bounds.
                let close = context.close_insolvent(
                    self.contract,
                    max_insurance_draw,
                    max_socialized_loss,
                    required_u64("KEEPER_HLP_MIN_BOUNTY_OUT")?,
                )?;
                if self.hlp_close_meets_bounds(&context, &snapshot, &close)? {
                    Ok(Some(("hlp_close_full", close)))
                } else {
                    Ok(None)
                }
            },
        )?;
        if let Some((action, instructions)) = recovered {
            // Submission errors may mean an unknown landed transaction. Never
            // select another recovery after starting the durable submit path.
            return self
                .submit_hlp(&context, &snapshot, action, instructions, true)
                .map(Some);
        }
        Ok(Some((
            None,
            "eligible but unfilled: hLP recovery did not meet configured payment, receipt, insurance, loss, or bounty bounds".into(),
        )))
    }

    fn hlp_flash_meets_bounds(
        &self,
        context: &HlpContext,
        snapshot: &Snapshot,
        instructions: &[Instruction],
        max_payment_debit: u64,
        min_collateral_credit: u64,
    ) -> Result<bool> {
        let post = [
            context.market.address,
            context.named["buyer_refund_account"],
            context.named["buyer_collateral_account"],
            context.named["session"],
            context.named["repayment_vault"],
        ];
        let value =
            simulation::simulate_for(self.client, context.buyer, snapshot, instructions, &post)?;
        if !value["value"]["err"].is_null() {
            return if expected_hlp_rejection(&value) || compute_limit_rejection(&value) {
                Ok(false)
            } else {
                Err(format!(
                    "unexpected hLP execution rejection: {}",
                    value["value"]
                ))
            };
        }
        if post_bytes(&value, 3)?.is_some() {
            return Err("hLP flash session did not close".into());
        }
        let payment_before = snapshot
            .accounts
            .get(&address(post[4]))
            .and_then(Option::as_ref)
            .and_then(|account| crate::liquidation::plan::token_amount(&account.data))
            .unwrap_or(0);
        let payment_after = post_bytes(&value, 4)?
            .as_deref()
            .and_then(crate::liquidation::plan::token_amount)
            .unwrap_or(0);
        if payment_after != payment_before {
            return Err("hLP payment vault did not return to its prior balance".into());
        }
        let debt_before = crate::liquidation::plan::token_amount(&snapshot.account(post[1])?.data)
            .ok_or("hLP buyer debt balance")?;
        let debt_after = crate::liquidation::plan::token_amount(
            &post_bytes(&value, 1)?.ok_or("missing hLP buyer debt post-state")?,
        )
        .ok_or("hLP buyer debt post balance")?;
        let collateral_before =
            crate::liquidation::plan::token_amount(&snapshot.account(post[2])?.data)
                .ok_or("hLP buyer collateral balance")?;
        let collateral_after = crate::liquidation::plan::token_amount(
            &post_bytes(&value, 2)?.ok_or("missing hLP buyer collateral post-state")?,
        )
        .ok_or("hLP buyer collateral post balance")?;
        if debt_before.saturating_sub(debt_after) > max_payment_debit
            || collateral_after
                .checked_sub(collateral_before)
                .is_none_or(|credit| credit < min_collateral_credit)
        {
            return Ok(false);
        }
        let market_after = post_bytes(&value, 0)?.ok_or("hLP recovery removed the market")?;
        Ok(market_after != snapshot.account(context.market.address)?.data)
    }

    fn hlp_close_meets_bounds(
        &self,
        context: &HlpContext,
        snapshot: &Snapshot,
        instructions: &[Instruction],
    ) -> Result<bool> {
        let post = [
            context.market.address,
            context.named["caller_bounty_account"],
        ];
        let value =
            simulation::simulate_for(self.client, context.buyer, snapshot, instructions, &post)?;
        if !value["value"]["err"].is_null() {
            return if expected_hlp_rejection(&value) || compute_limit_rejection(&value) {
                Ok(false)
            } else {
                Err(format!(
                    "unexpected hLP close rejection: {}",
                    value["value"]
                ))
            };
        }
        let before = crate::liquidation::plan::token_amount(&snapshot.account(post[1])?.data)
            .ok_or("hLP bounty account balance")?;
        let after = crate::liquidation::plan::token_amount(
            &post_bytes(&value, 1)?.ok_or("missing hLP bounty post-state")?,
        )
        .ok_or("hLP bounty post balance")?;
        if after.saturating_sub(before) < required_u64("KEEPER_HLP_MIN_BOUNTY_OUT")? {
            return Ok(false);
        }
        let market_after = post_bytes(&value, 0)?.ok_or("hLP close removed the market")?;
        Ok(market_after != snapshot.account(context.market.address)?.data)
    }

    fn submit_hlp(
        &self,
        context: &HlpContext,
        snapshot: &Snapshot,
        action: &str,
        instructions: Vec<Instruction>,
        recovery: bool,
    ) -> Result<(Option<String>, String)> {
        if self.dry_run {
            if !recovery
                && !simulation::simulate_for(
                    self.client,
                    context.buyer,
                    snapshot,
                    &instructions,
                    &[],
                )?["value"]["err"]
                    .is_null()
            {
                return Err("shadow hLP setup simulation rejected".into());
            }
            return Ok((
                None,
                format!("shadow {action}: simulated; no signature produced"),
            ));
        }
        let (fresh, current) = self.fresh_hlp(context.market.address, context.target_asset)?;
        if state_hash(snapshot, context.epoch)? != state_hash(&current, fresh.epoch)? {
            return Err("hLP state or fee epoch changed; requote".into());
        }
        if recovery {
            if action.starts_with("hlp_flash") {
                if !self.hlp_flash_meets_bounds(
                    &fresh,
                    &current,
                    &instructions,
                    required_u64("KEEPER_HLP_MAX_PAYMENT_DEBIT")?,
                    required_u64("KEEPER_HLP_MIN_COLLATERAL_CREDIT")?,
                )? {
                    return Err("hLP flash no longer meets bounds".into());
                }
            } else if !self.hlp_close_meets_bounds(&fresh, &current, &instructions)? {
                return Err("hLP close no longer meets bounds".into());
            }
        } else if !simulation::simulate_for(self.client, fresh.buyer, &current, &instructions, &[])?
            ["value"]["err"]
            .is_null()
        {
            return Err("hLP pre-submit setup simulation rejected".into());
        }
        let block: serde_json::Value = self
            .client
            .call(
                "getLatestBlockhash",
                serde_json::json!([{"commitment":"confirmed","minContextSlot":current.slot}]),
            )
            .map_err(|error| error.to_string())?;
        let hash = block["value"]["blockhash"]
            .as_str()
            .ok_or("blockhash missing")?;
        let height = block["value"]["lastValidBlockHeight"]
            .as_u64()
            .ok_or("block height missing")?;
        let message =
            simulation::compile_message_for(fresh.buyer, &current, &instructions, key(hash)?)?;
        if serialize_transaction(&[[0; 64]], &message).len() > 1232 {
            return Err("hLP transaction exceeds packet limit".into());
        }
        let mut journal = Journal::connect(
            &std::env::var("DATABASE_URL")
                .map_err(|_| "DATABASE_URL is required before signing")?,
        )?;
        let signature = journal.submit(
            self.client,
            self.signer,
            &self.contract.protocol_revision,
            &address(fresh.market.address),
            fresh.target_asset,
            &address(fresh.market.address),
            action,
            current.slot,
            &state_hash(&current, fresh.epoch)?,
            &message,
            hash,
            height,
        )?;
        Ok((
            signature,
            format!("{action}: journaled; pending signatures are reconciled before replacement"),
        ))
    }
}
// A failed quote, payment gross-up, or flash simulation must still permit
// terminal hLP recovery. Preserve that error if terminal recovery is unavailable.
fn try_hlp_recovery<T>(
    flash: impl FnOnce() -> Result<Option<T>>,
    terminal: impl FnOnce() -> Result<Option<T>>,
) -> Result<Option<T>> {
    let flash_result = flash();
    if let Ok(Some(value)) = flash_result {
        return Ok(Some(value));
    }
    if let Some(value) = terminal()? {
        return Ok(Some(value));
    }
    flash_result
}

fn expected_hlp_rejection(value: &serde_json::Value) -> bool {
    let Some(logs) = value["value"]["logs"].as_array() else {
        return false;
    };
    [
        "HlpSettlementUnavailable",
        "HlpNotLiquidatable",
        "SlippageExceeded",
    ]
    .iter()
    .any(|name| {
        logs.iter().any(|log| {
            log.as_str()
                .is_some_and(|line| line.contains(&format!("Error Code: {name}.")))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_recovery_survives_flash_plan_construction_failure() {
        let calls = std::cell::RefCell::new(Vec::new());
        let recovered = try_hlp_recovery(
            || {
                calls.borrow_mut().push("flash plan");
                Err("payment cannot be grossed up".into())
            },
            || {
                calls.borrow_mut().push("terminal simulation");
                Ok(Some("hlp_close_full"))
            },
        )
        .unwrap();
        assert_eq!(recovered, Some("hlp_close_full"));
        assert_eq!(*calls.borrow(), ["flash plan", "terminal simulation"]);
        assert_eq!(
            try_hlp_recovery::<()>(|| Err("invalid flash quote".into()), || Ok(None)),
            Err("invalid flash quote".into())
        );
        assert_eq!(
            try_hlp_recovery(|| Ok(Some("hlp_flash")), || panic!("already filled")),
            Ok(Some("hlp_flash"))
        );
    }
}
