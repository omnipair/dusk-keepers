//! Shared position-liquidation engine used by lending and leverage workers.
mod jupiter;
use super::{
    policy::{LiquidationMode, select_liquidation},
    position_log,
    search::search_profitable,
    simulation,
    validation::*,
};
use crate::runtime::flash_journal::recover_pending;
use crate::{
    liquidation::plan::{Context, Snapshot, address, key},
    runtime::execution::{AttemptReport, ExecutionError},
    runtime::flash_journal::Journal,
    runtime::rpc::{RpcClient, account_discriminator},
    runtime::signer::TransactionSigner,
    runtime::transaction::{Instruction, serialize_transaction},
};
use borsh::BorshDeserialize;
use dusk_adapter::{
    AccountLayoutManifest, InstructionContract,
    liquidation::{EmergencyLiquidationQuote, FlashLiquidationQuote, LiquidationHealth},
    transfer_fees::CLOCK,
};
use keeper_core::{OutcomeStatus, ReasonCode};
use sha2::Digest;
use std::collections::BTreeSet;
type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy)]
pub enum PositionKind {
    Borrowing,
    Leverage,
}

pub struct LiquidationJob<'a> {
    pub client: &'a RpcClient,
    pub layout: &'a AccountLayoutManifest,
    pub contract: &'a InstructionContract,
    pub signer: &'a dyn TransactionSigner,
    pub dry_run: bool,
    pub kind: PositionKind,
    pub liquidation_mode: LiquidationMode,
    pub max_sends: usize,
    pub priority_markets: &'a BTreeSet<[u8; 32]>,
}
struct LiquidationPlan {
    action: &'static str,
    instructions: Vec<Instruction>,
    minimum_profit: u64,
    route_state: Option<jupiter::RouteState>,
}

impl LiquidationJob<'_> {
    pub fn run_pass(&self) -> std::result::Result<Vec<AttemptReport>, ExecutionError> {
        let mut reports =
            recover_pending(self.client, &self.contract.protocol_revision, self.dry_run)?;
        let pending = if self.dry_run {
            BTreeSet::new()
        } else {
            Journal::connect(&std::env::var("DATABASE_URL").map_err(|_| {
                ExecutionError::Assembly("DATABASE_URL is required before signing".into())
            })?)
            .and_then(|mut j| j.pending_sides(&self.contract.protocol_revision))
            .map_err(ExecutionError::Assembly)?
        };
        let kind = match self.kind {
            PositionKind::Borrowing => "BorrowPosition",
            PositionKind::Leverage => "LeveragePosition",
        };
        let program = &self.layout.program_id;
        let reader = self
            .layout
            .reader(kind)
            .map_err(|e| ExecutionError::Assembly(e.to_string()))?;
        let active_markets: BTreeSet<_> = self
            .client
            .program_accounts(program, account_discriminator("Market"))?
            .into_iter()
            .filter_map(|(address, data)| {
                crate::runtime::accounts::MarketAccounts::decode(self.layout, address, &data)
                    .map(|_| address)
            })
            .collect();
        let mut positions = self
            .client
            .program_accounts(program, account_discriminator(kind))?;
        positions.sort_by_key(|(_, data)| {
            !reader
                .pubkey("market", data)
                .ok()
                .is_some_and(|k| self.priority_markets.contains(&k))
        });
        let mut attempted = 0;
        let mut jupiter_budget = if self.liquidation_mode != LiquidationMode::Emergency
            && settlement().map_err(ExecutionError::Assembly)? == Settlement::Jupiter
        {
            crate::routing::jupiter::Config::pass_budget().map_err(ExecutionError::Assembly)?
        } else {
            0
        };
        if position_log::enabled() {
            println!(
                "position_scan kind={kind} discovered={} execution_scope=all_supported_markets",
                positions.len()
            );
        }
        for (position, data) in positions {
            // Identity is a fixed owner/market prefix shared by wallet account
            // types. Full-layout readers intentionally reject retired shapes,
            // so scope discovery before invoking those readers.
            let market: [u8; 32] = data
                .get(40..72)
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| {
                    ExecutionError::Assembly("position identity prefix is truncated".into())
                })?;
            let owner = data.get(8..40).and_then(|v| v.try_into().ok());
            if !active_markets.contains(&market) {
                position_log::unavailable(
                    position,
                    market,
                    owner,
                    kind,
                    "market is missing or unsupported; no health evaluation or execution",
                );
                continue;
            }
            let sides = if kind == "LeveragePosition" {
                vec![
                    reader
                        .u8("debt_asset", &data)
                        .map_err(|e| ExecutionError::Assembly(e.to_string()))?,
                ]
            } else {
                vec![0, 1]
            };
            for side in sides {
                if pending.contains(&(position, side)) {
                    position_log::decision(
                        position,
                        side,
                        "pending signature: awaiting reconciliation; no new route requested",
                        None,
                    );
                    continue;
                }
                let limit_reached = send_limit_reached(attempted, self.max_sends);
                if limit_reached {
                    if !position_log::enabled() {
                        return Ok(reports);
                    }
                    let reason = "MONITOR ONLY: transaction budget used for this pass";
                    // Diagnostics only simulate health. No route selection,
                    // signing or journal submission occurs after the pass limit.
                    if let Err(error) = self.inspect_health(position, kind, side, Some(reason)) {
                        position_log::unavailable(
                            position,
                            market,
                            owner,
                            kind,
                            &format!("{reason}; {error}"),
                        );
                    }
                    continue;
                }
                match self.attempt(position, kind, side, &mut jupiter_budget) {
                    Ok(Some((signature, detail))) => {
                        position_log::decision(position, side, &detail, signature.as_deref());
                        if counts_toward_send_limit(&detail) {
                            attempted += 1;
                        }
                        reports.push(AttemptReport {
                            position: address(position),
                            debt_mint: String::new(),
                            status: if signature.is_some() {
                                OutcomeStatus::Executed
                            } else {
                                OutcomeStatus::Skipped
                            },
                            reason: if signature.is_some() {
                                ReasonCode::Confirmed
                            } else if !self.dry_run && counts_toward_send_limit(&detail) {
                                ReasonCode::StateChanged
                            } else {
                                ReasonCode::BoundsNotMet
                            },
                            race: None,
                            signature,
                            detail: Some(detail),
                        });
                    }
                    Ok(None) => {}
                    Err(e) => {
                        position_log::decision(position, side, &e, None);
                        attempted += 1;
                        reports.push(AttemptReport {
                            position: address(position),
                            debt_mint: String::new(),
                            status: OutcomeStatus::RetryableFailure,
                            reason: ReasonCode::SimulationRejected,
                            race: None,
                            signature: None,
                            detail: Some(e),
                        });
                    }
                }
            }
        }
        Ok(reports)
    }
    fn fresh(&self, position: [u8; 32], kind: &str, side: u8) -> Result<(Context, Snapshot)> {
        self.fresh_with_extra(position, kind, side, &[])
    }
    fn fresh_with_extra(
        &self,
        position: [u8; 32],
        kind: &str,
        side: u8,
        extra: &[[u8; 32]],
    ) -> Result<(Context, Snapshot)> {
        let r = self.layout.reader(kind).map_err(|e| e.to_string())?;
        let first = Snapshot::fetch(self.client, &[position], 0).map_err(|e| e.to_string())?;
        let market = r
            .pubkey(
                "market",
                first.owned(position, key(&self.layout.program_id)?)?,
            )
            .map_err(|e| e.to_string())?;
        let second = Snapshot::fetch(self.client, &[position, market], first.slot)
            .map_err(|e| e.to_string())?;
        let m = crate::runtime::accounts::MarketAccounts::decode(
            self.layout,
            market,
            second.owned(market, key(&self.layout.program_id)?)?,
        )
        .ok_or("invalid market")?;
        let keys = [position, market, m.base_mint, m.quote_mint, key(CLOCK)?];
        let third = Snapshot::fetch(self.client, &keys, second.slot).map_err(|e| e.to_string())?;
        let c = Context::resolve(
            self.layout,
            &third,
            position,
            self.signer.public_key(),
            kind,
            side,
        )?;
        let mut all = keys.to_vec();
        all.extend(c.named.values());
        all.extend(c.market.hlp_remaining_accounts().iter().map(|a| a.pubkey));
        all.extend(
            c.market
                .oracle_remaining_accounts()
                .iter()
                .map(|a| a.pubkey),
        );
        all.sort();
        all.dedup();
        // Executable/static keys are not needed in the semantic state snapshot.
        all.retain(|k| *k != c.program && *k != c.named["event_authority"]);
        if let Ok(table) = std::env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
            all.push(key(&table)?);
            all.sort();
            all.dedup();
        }
        all.extend_from_slice(extra);
        all.sort();
        all.dedup();
        let snapshot = Snapshot::fetch(self.client, &all, third.slot).map_err(|e| e.to_string())?;
        let c = Context::resolve(
            self.layout,
            &snapshot,
            position,
            self.signer.public_key(),
            kind,
            side,
        )?;
        for feed in c.market.oracle_remaining_accounts() {
            snapshot.account(feed.pubkey)?;
        }
        Ok((c, snapshot))
    }
    fn message(
        &self,
        c: &Context,
        s: &Snapshot,
        ixs: &[Instruction],
        hash: [u8; 32],
    ) -> Result<Vec<u8>> {
        simulation::compile_message_for(c.buyer, s, ixs, hash)
    }
    fn simulate(
        &self,
        c: &Context,
        snapshot: &Snapshot,
        ixs: &[Instruction],
        post: &[[u8; 32]],
    ) -> Result<serde_json::Value> {
        simulation::simulate_for(self.client, c.buyer, snapshot, ixs, post)
    }
    fn preview<T: BorshDeserialize>(
        &self,
        c: &Context,
        s: &Snapshot,
        ix: Instruction,
    ) -> Result<Option<T>> {
        let mut instructions = crate::liquidation::plan::resources()?;
        instructions.push(ix);
        let value = self.simulate(c, s, &instructions, &[])?;
        if !value["value"]["err"].is_null() {
            if compute_limit_rejection(&value) {
                eprintln!(
                    "keeper liquidation probe exceeded transaction compute limit; trying other permitted sizes"
                );
                return Ok(None);
            }
            return if expected_bound_rejection(&value) || zero_share_probe(&value) {
                Ok(None)
            } else {
                Err(format!("unexpected preview rejection: {}", value["value"]))
            };
        }
        crate::liquidation::plan::decode_return(&value["value"]["returnData"], c.program).map(Some)
    }
    fn inspect_health(
        &self,
        position: [u8; 32],
        kind: &str,
        side: u8,
        monitor_reason: Option<&str>,
    ) -> Result<(Context, Snapshot, Option<LiquidationHealth>)> {
        // A load-balanced RPC may transiently pair a Clock from a different
        // bank with the account context. Retry the entire snapshot, never
        // relax the equality check or merge accounts from separate reads.
        let mut snapshot = self.fresh(position, kind, side);
        for _ in 0..2 {
            if snapshot.as_ref().err().map(String::as_str)
                != Some("clock and RPC bank do not match")
            {
                break;
            }
            snapshot = self.fresh(position, kind, side);
        }
        let (c, s) = snapshot?;
        if c.shares == 0 {
            position_log::health(&c, &s, None, None, monitor_reason);
            return Ok((c, s, None));
        }
        let mut preview = crate::liquidation::plan::resources()?;
        preview.push(c.health_preview(self.contract)?);
        let result = self.simulate(&c, &s, &preview, &[])?;
        if !result["value"]["err"].is_null() {
            return Err(format!(
                "health preview rejected: {}",
                result["value"]["err"]
            ));
        }
        let health: LiquidationHealth =
            crate::liquidation::plan::decode_return(&result["value"]["returnData"], c.program)?;
        position_log::health(
            &c,
            &s,
            Some(&health),
            result["context"]["slot"].as_u64(),
            monitor_reason,
        );
        Ok((c, s, Some(health)))
    }
    fn attempt(
        &self,
        position: [u8; 32],
        kind: &str,
        side: u8,
        jupiter_budget: &mut usize,
    ) -> Result<Option<(Option<String>, String)>> {
        let (c, s, health) = self.inspect_health(position, kind, side, None)?;
        let Some(health) = health else {
            return Ok(None);
        };
        if !health.eligible() {
            return Ok(None);
        }
        let setup = c.setup(&s)?;
        if !setup.is_empty() {
            return self
                .submit(&c, &s, "prepare_accounts", setup, 0, None)
                .map(Some);
        }
        let plan = select_liquidation(
            self.liquidation_mode,
            || self.ordinary_plan(&c, &s, &health, jupiter_budget),
            || self.emergency_plan(&c, &s),
        )?;
        if let Some(plan) = plan {
            // Once a plan enters submission, every outcome returns to the journal.
            // Submission errors and unknown signatures never select another route.
            return self
                .submit(
                    &c,
                    plan.route_state.as_ref().map_or(&s, |r| &r.snapshot),
                    plan.action,
                    plan.instructions,
                    plan.minimum_profit,
                    plan.route_state.as_ref(),
                )
                .map(Some);
        }
        Ok(Some((None, "eligible but unfilled: no adaptively searched permitted route met the configured payment/profit bounds".into())))
    }

    fn ordinary_plan(
        &self,
        c: &Context,
        s: &Snapshot,
        health: &LiquidationHealth,
        jupiter_budget: &mut usize,
    ) -> Result<Option<LiquidationPlan>> {
        if settlement()? == Settlement::Jupiter {
            return self.jupiter_plan(c, s, health, jupiter_budget);
        }
        let inventory = self.inventory_mode()?;
        if inventory {
            let market = std::env::var("KEEPER_FLASH_MARKET")
                .map_err(|_| "KEEPER_FLASH_MARKET is required for inventory settlement")?;
            key(&market)?;
            if market != address(c.market.address) {
                return Ok(None);
            }
        }
        let minimum_profit = if inventory {
            0
        } else {
            required_u64("KEEPER_LIQUIDATION_MIN_PROFIT")?
        };
        let cap = required_u64("KEEPER_FLASH_MAX_REPAY")?;
        if cap == 0 {
            return Err("KEEPER_FLASH_MAX_REPAY must be positive".into());
        }
        let partial = search_profitable(cap.min(health.debt), minimum_profit, |repay| {
            let Some(q) = self.preview::<FlashLiquidationQuote>(
                c,
                s,
                c.preview_flash(self.contract, repay, false)?,
            )?
            else {
                return Ok(None);
            };
            // Score without the final floor, then rebuild and recheck the winner
            // with that floor enforced on chain.
            let Some(ixs) = self.flash_plan(c, s, &q, repay, 0)? else {
                return Ok(None);
            };
            Ok(self
                .liquidation_profit(c, s, &ixs, q.full)?
                .map(|profit| (profit, (repay, q))))
        })?;
        if let Some((repay, quote)) = partial {
            let instructions = self
                .flash_plan(c, s, &quote, repay, minimum_profit)?
                .ok_or("selected flash route no longer meets configured bounds")?;
            if !self.profitable(c, s, &instructions, minimum_profit, false)? {
                return Err("selected flash route failed protected revalidation".into());
            }
            return Ok(Some(LiquidationPlan {
                action: "flash",
                instructions,
                minimum_profit,
                route_state: None,
            }));
        }
        if cap >= health.debt
            && let Some(q) = self.preview::<FlashLiquidationQuote>(
                c,
                s,
                c.preview_flash(self.contract, cap, true)?,
            )?
            && let Some(instructions) = self.flash_plan(c, s, &q, cap, minimum_profit)?
            && self.profitable(c, s, &instructions, minimum_profit, q.full)?
        {
            return Ok(Some(LiquidationPlan {
                action: "flash_full",
                instructions,
                minimum_profit,
                route_state: None,
            }));
        }
        Ok(None)
    }

    fn emergency_plan(&self, c: &Context, s: &Snapshot) -> Result<Option<LiquidationPlan>> {
        let minimum_profit = required_u64("KEEPER_LIQUIDATION_MIN_PROFIT")?;
        let slippage_bps = std::env::var("KEEPER_EMERGENCY_SLIPPAGE_BPS")
            .unwrap_or_else(|_| "0".into())
            .parse::<u16>()
            .map_err(|_| "invalid KEEPER_EMERGENCY_SLIPPAGE_BPS")?;
        // Validate even when no quote is available. The default retains the
        // exact quoted limits; an operator must opt in to quote movement.
        emergency_slippage_floor(0, slippage_bps)?;
        let partial =
            search_profitable(c.collateral.saturating_sub(1), minimum_profit, |amount| {
                let Some(q) = self.preview::<EmergencyLiquidationQuote>(
                    c,
                    s,
                    c.preview_emergency(self.contract, amount, false)?,
                )?
                else {
                    return Ok(None);
                };
                let instructions = self.emergency_instructions(c, &q, slippage_bps, 0)?;
                Ok(self
                    .liquidation_profit(c, s, &instructions, false)?
                    .map(|profit| (profit, q)))
            })?;
        if let Some(quote) = partial {
            // Score below-floor candidates during adaptive search, then put
            // the configured profit floor into the chosen transaction itself.
            let instructions =
                self.emergency_instructions(c, &quote, slippage_bps, minimum_profit)?;
            if !self.profitable(c, s, &instructions, minimum_profit, false)? {
                return Err("selected emergency route failed protected revalidation".into());
            }
            return Ok(Some(LiquidationPlan {
                action: "emergency",
                instructions,
                minimum_profit,
                route_state: None,
            }));
        }
        // A failed partial search never authorizes a full close. The program
        // must independently accept the full emergency preview.
        if let Some(q) = self.preview::<EmergencyLiquidationQuote>(
            c,
            s,
            c.preview_emergency(self.contract, c.collateral, true)?,
        )? {
            let instructions = self.emergency_instructions(c, &q, slippage_bps, minimum_profit)?;
            if self.profitable(c, s, &instructions, minimum_profit, true)? {
                return Ok(Some(LiquidationPlan {
                    action: "emergency_full",
                    instructions,
                    minimum_profit,
                    route_state: None,
                }));
            }
        }
        Ok(None)
    }
    fn emergency_instructions(
        &self,
        c: &Context,
        q: &EmergencyLiquidationQuote,
        slippage_bps: u16,
        minimum_profit: u64,
    ) -> Result<Vec<Instruction>> {
        let mut instructions = crate::liquidation::plan::resources()?;
        instructions.push(c.emergency_with_bounds(
            self.contract,
            q,
            emergency_slippage_floor(q.reward_credit, slippage_bps)?.max(minimum_profit),
            emergency_slippage_floor(q.swap_output, slippage_bps)?,
        )?);
        Ok(instructions)
    }
    fn inventory_mode(&self) -> Result<bool> {
        Ok(settlement()? == Settlement::Inventory)
    }

    fn flash_plan(
        &self,
        c: &Context,
        s: &Snapshot,
        q: &FlashLiquidationQuote,
        repayment: u64,
        min_profit: u64,
    ) -> Result<Option<Vec<Instruction>>> {
        if self.inventory_mode()? {
            let (payment, gross) = crate::liquidation::plan::inventory_payment(c, s, q.payment)?;
            if gross > required_u64("KEEPER_FLASH_MAX_PAYMENT_DEBIT")?
                || q.collateral_credit < required_u64("KEEPER_FLASH_MIN_COLLATERAL_CREDIT")?
            {
                return Ok(None);
            }
            c.flash(
                self.contract,
                q,
                crate::liquidation::plan::FlashPlanOptions {
                    max_repayment: repayment,
                    prefix: crate::liquidation::plan::resources()?,
                    route: vec![payment],
                    internal: false,
                    min_buyer_credit: 0,
                },
            )
            .map(Some)
        } else {
            c.flash(
                self.contract,
                q,
                crate::liquidation::plan::FlashPlanOptions {
                    max_repayment: repayment,
                    prefix: crate::liquidation::plan::resources()?,
                    route: vec![],
                    internal: true,
                    min_buyer_credit: min_profit,
                },
            )
            .map(Some)
        }
    }
    fn profitable(
        &self,
        c: &Context,
        s: &Snapshot,
        ixs: &[Instruction],
        minimum: u64,
        full: bool,
    ) -> Result<bool> {
        Ok(self
            .liquidation_profit(c, s, ixs, full)?
            .is_some_and(|profit| profit >= minimum))
    }
    fn liquidation_profit(
        &self,
        c: &Context,
        s: &Snapshot,
        ixs: &[Instruction],
        full: bool,
    ) -> Result<Option<u64>> {
        let jupiter = ixs
            .iter()
            .any(|i| address(i.program_id) == crate::routing::jupiter::PROGRAM);
        let mut post = vec![
            c.position,
            c.named["buyer_refund_account"],
            c.named["buyer_collateral_account"],
            c.named["session"],
            c.named["repayment_vault"],
        ];
        if jupiter {
            post.push(c.buyer);
        }
        let value = self.simulate(c, s, ixs, &post)?;
        if !value["value"]["err"].is_null() {
            if compute_limit_rejection(&value) {
                eprintln!(
                    "keeper liquidation execution exceeded transaction compute limit; trying other permitted sizes"
                );
                return Ok(None);
            }
            return if expected_bound_rejection(&value)
                || (jupiter && jupiter::slippage_rejection(&value, ixs))
            {
                Ok(None)
            } else {
                Err(format!(
                    "unexpected execution rejection: {}",
                    value["value"]
                ))
            };
        }
        if jupiter {
            let before = s
                .account(c.buyer)?
                .lamports
                .ok_or("missing payer SOL balance")?;
            let after = value["value"]["accounts"][5]["lamports"]
                .as_u64()
                .ok_or("missing payer SOL post-state")?;
            if before.saturating_sub(after)
                > crate::routing::jupiter::Config::from_env()?.max_sol_debit
            {
                return Err("Jupiter transaction exceeds the separate SOL spending budget".into());
            }
        }
        if post_bytes(&value, 3)?.is_some() {
            return Err("flash session did not close".into());
        }
        let payment_before = s
            .accounts
            .get(&address(post[4]))
            .and_then(Option::as_ref)
            .and_then(|a| crate::liquidation::plan::token_amount(&a.data))
            .unwrap_or(0);
        let payment_after = post_bytes(&value, 4)?
            .as_deref()
            .and_then(crate::liquidation::plan::token_amount)
            .unwrap_or(0);
        if payment_after != payment_before {
            return Err("isolated payment vault did not return to its prior balance".into());
        }
        let debt_before = crate::liquidation::plan::token_amount(&s.account(post[1])?.data)
            .ok_or("debt token balance")?;
        let debt_after = crate::liquidation::plan::token_amount(
            &post_bytes(&value, 1)?.ok_or("missing debt token post-state")?,
        )
        .ok_or("debt token post balance")?;
        let collateral_before = crate::liquidation::plan::token_amount(&s.account(post[2])?.data)
            .ok_or("collateral balance")?;
        let collateral_after = crate::liquidation::plan::token_amount(
            &post_bytes(&value, 2)?.ok_or("missing collateral post-state")?,
        )
        .ok_or("collateral post balance")?;
        let is_flash = ixs.iter().any(|i| {
            i.data.get(..8)
                == Some(sha2::Sha256::digest(b"global:begin_flash_liquidation")[..8].as_ref())
        });
        let inventory = is_flash && self.inventory_mode()?;
        if inventory {
            let cap = required_u64("KEEPER_FLASH_MAX_PAYMENT_DEBIT")?;
            let min_credit = required_u64("KEEPER_FLASH_MIN_COLLATERAL_CREDIT")?;
            if debt_before.saturating_sub(debt_after) > cap
                || collateral_after
                    .checked_sub(collateral_before)
                    .is_none_or(|v| v < min_credit)
            {
                return Ok(None);
            }
        }
        let profit = if inventory {
            0
        } else {
            let Some(profit) = debt_after.checked_sub(debt_before) else {
                return Ok(None);
            };
            if collateral_after < collateral_before {
                return Ok(None);
            }
            profit
        };
        let Some(position_after) = post_bytes(&value, 0)? else {
            return Ok(full.then_some(profit));
        };
        let r = self.layout.reader(&c.kind).map_err(|e| e.to_string())?;
        let field = if c.kind == "LeveragePosition" {
            "debt_shares"
        } else if c.debt_asset == 0 {
            "fixed_base_shares"
        } else {
            "fixed_quote_shares"
        };
        let remaining = u128::from_le_bytes(
            r.bytes(field, &position_after)
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "shares post length")?,
        );
        Ok((remaining < c.shares && (!full || remaining == 0)).then_some(profit))
    }
    fn submit(
        &self,
        c: &Context,
        s: &Snapshot,
        action: &str,
        ixs: Vec<Instruction>,
        minimum: u64,
        route_state: Option<&jupiter::RouteState>,
    ) -> Result<(Option<String>, String)> {
        let liquidation = action != "prepare_accounts";
        jupiter::check_deadline(route_state)?;
        if self.dry_run {
            if !liquidation && !self.simulate(c, s, &ixs, &[])?["value"]["err"].is_null() {
                return Err("shadow account-preparation simulation rejected".into());
            }
            return Ok((
                None,
                format!("shadow {action}: simulated; no signature produced"),
            ));
        }
        // Re-read all safety-critical state, including both schedules and Clock.
        let extra = if route_state.is_some() {
            s.accounts
                .keys()
                .map(|k| key(k))
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![]
        };
        let (fresh, now) = self.fresh_with_extra(c.position, &c.kind, c.debt_asset, &extra)?;
        if state_hash(s, c.epoch)? != state_hash(&now, fresh.epoch)? {
            return Err("state or fee epoch changed; requote".into());
        }
        if liquidation && !self.profitable(&fresh, &now, &ixs, minimum, action.ends_with("full"))? {
            return Err("execution no longer meets bounds".into());
        }
        if !liquidation && !self.simulate(&fresh, &now, &ixs, &[])?["value"]["err"].is_null() {
            return Err("pre-submit simulation rejected".into());
        }
        let block: serde_json::Value = self
            .client
            .call(
                "getLatestBlockhash",
                serde_json::json!([{"commitment":"confirmed","minContextSlot":now.slot}]),
            )
            .map_err(|e| e.to_string())?;
        let hash = block["value"]["blockhash"]
            .as_str()
            .ok_or("blockhash missing")?;
        let height = block["value"]["lastValidBlockHeight"]
            .as_u64()
            .ok_or("block height missing")?;
        let message = self.message(c, &now, &ixs, key(hash)?)?;
        if serialize_transaction(&[[0; 64]], &message).len() > 1232 {
            return Err("transaction exceeds packet limit".into());
        }
        let mut journal = Journal::connect(
            &std::env::var("DATABASE_URL")
                .map_err(|_| "DATABASE_URL is required before signing")?,
        )?;
        jupiter::check_deadline(route_state)?;
        let signature = journal.submit_before(
            self.client,
            self.signer,
            &self.contract.protocol_revision,
            &address(c.position),
            c.debt_asset,
            &address(c.market.address),
            action,
            now.slot,
            &state_hash(&now, fresh.epoch)?,
            &message,
            hash,
            height,
            route_state.map(|state| state.expires_at),
        )?;
        Ok((
            signature,
            format!("{action}: journaled; pending signatures are reconciled before replacement"),
        ))
    }
}

fn send_limit_reached(attempted: usize, maximum: usize) -> bool {
    attempted >= maximum
}

fn counts_toward_send_limit(detail: &str) -> bool {
    !detail.starts_with("eligible but unfilled:")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unfilled_positions_do_not_consume_transaction_send_budget() {
        assert!(send_limit_reached(4, 4));
        assert!(counts_toward_send_limit(
            "shadow flash: simulated; no signature produced"
        ));
        assert!(!counts_toward_send_limit("eligible but unfilled: no route"));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Settlement {
    Jupiter,
    Dusk,
    Inventory,
}
fn settlement() -> Result<Settlement> {
    match std::env::var("KEEPER_FLASH_SETTLEMENT")
        .unwrap_or_else(|_| "jupiter".into())
        .as_str()
    {
        "jupiter" => Ok(Settlement::Jupiter),
        "dusk" => Ok(Settlement::Dusk),
        "inventory" => Ok(Settlement::Inventory),
        _ => Err("KEEPER_FLASH_SETTLEMENT must be jupiter, dusk or inventory".into()),
    }
}
