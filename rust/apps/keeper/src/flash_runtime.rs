//! Native RPC health previews, atomic flash fills and emergency settlement.
use crate::{
    discovery::{RpcClient, account_discriminator},
    execute::{AttemptReport, ExecutionError},
    flash::{self, Context, Snapshot, address, key},
    flash_journal::Journal,
    hlp_flash::HlpContext,
    signer::TransactionSigner,
    transaction::{Instruction, base64, compile_message, serialize_transaction},
};
use borsh::BorshDeserialize;
use dusk_adapter::{
    AccountLayoutManifest, InstructionContract,
    liquidation::{
        EmergencyLiquidationQuote, FlashLiquidationQuote, HlpFlashQuote, LiquidationHealth,
    },
    transfer_fees::CLOCK,
};
use keeper_core::{OutcomeStatus, ReasonCode};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Included},
};
type Result<T> = std::result::Result<T, String>;
pub struct FlashJob<'a> {
    pub client: &'a RpcClient,
    pub layout: &'a AccountLayoutManifest,
    pub contract: &'a InstructionContract,
    pub signer: &'a dyn TransactionSigner,
    pub dry_run: bool,
    pub profile: &'a str,
    pub max_sends: usize,
    pub priority_markets: &'a BTreeSet<[u8; 32]>,
}
impl FlashJob<'_> {
    pub fn run_pass(&self) -> std::result::Result<Vec<AttemptReport>, ExecutionError> {
        let mut reports = Vec::new();
        if !self.dry_run && self.profile != "lending-trigger" {
            let mut journal = Journal::connect(&std::env::var("DATABASE_URL").map_err(|_| {
                ExecutionError::Assembly("DATABASE_URL is required before signing".into())
            })?)
            .map_err(ExecutionError::Assembly)?;
            for (position, signature, outcome) in journal
                .recover(self.client, &self.contract.protocol_revision)
                .map_err(ExecutionError::Assembly)?
            {
                let (status, reason) = recovered_submission_status(&outcome);
                reports.push(AttemptReport {
                    position,
                    debt_mint: String::new(),
                    status,
                    reason,
                    race: None,
                    signature,
                    detail: Some(outcome),
                });
            }
        }
        if self.profile == "hlp-recovery" {
            return self.run_hlp_pass(reports);
        }
        let kind = if self.profile == "leverage" {
            "LeveragePosition"
        } else {
            "BorrowPosition"
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
                crate::accounts::MarketAccounts::decode(self.layout, address, &data)
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
            if !active_markets.contains(&market) {
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
                if send_limit_reached(self.profile, attempted, self.max_sends) {
                    return Ok(reports);
                }
                match self.attempt(position, kind, side) {
                    Ok(Some((signature, detail))) => {
                        if counts_toward_send_limit(self.profile, &detail) {
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
                        if self.profile != "lending-trigger" {
                            attempted += 1;
                        }
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
    fn run_hlp_pass(
        &self,
        mut reports: Vec<AttemptReport>,
    ) -> std::result::Result<Vec<AttemptReport>, ExecutionError> {
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
        let decoded = crate::accounts::MarketAccounts::decode(
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
        let mut instructions = flash::resources()?;
        instructions.push(context.preview(self.contract)?);
        let value = self.simulate_for(context.buyer, snapshot, &instructions, &[])?;
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
        flash::decode_return(&value["value"]["returnData"], context.program).map(Some)
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
                    .and_then(|account| flash::token_amount(&account.data))
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
        let value = self.simulate_for(context.buyer, snapshot, instructions, &post)?;
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
            .and_then(|account| flash::token_amount(&account.data))
            .unwrap_or(0);
        let payment_after = post_bytes(&value, 4)?
            .as_deref()
            .and_then(flash::token_amount)
            .unwrap_or(0);
        if payment_after != payment_before {
            return Err("hLP payment vault did not return to its prior balance".into());
        }
        let debt_before = flash::token_amount(&snapshot.account(post[1])?.data)
            .ok_or("hLP buyer debt balance")?;
        let debt_after = flash::token_amount(
            &post_bytes(&value, 1)?.ok_or("missing hLP buyer debt post-state")?,
        )
        .ok_or("hLP buyer debt post balance")?;
        let collateral_before = flash::token_amount(&snapshot.account(post[2])?.data)
            .ok_or("hLP buyer collateral balance")?;
        let collateral_after = flash::token_amount(
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
        let value = self.simulate_for(context.buyer, snapshot, instructions, &post)?;
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
        let before = flash::token_amount(&snapshot.account(post[1])?.data)
            .ok_or("hLP bounty account balance")?;
        let after =
            flash::token_amount(&post_bytes(&value, 1)?.ok_or("missing hLP bounty post-state")?)
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
                && !self.simulate_for(context.buyer, snapshot, &instructions, &[])?["value"]["err"]
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
        } else if !self.simulate_for(fresh.buyer, &current, &instructions, &[])?["value"]["err"]
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
        let message = Self::compile_message_for(fresh.buyer, &current, &instructions, key(hash)?)?;
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
    fn fresh(&self, position: [u8; 32], kind: &str, side: u8) -> Result<(Context, Snapshot)> {
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
        let m = crate::accounts::MarketAccounts::decode(
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
        all.sort();
        all.dedup();
        // Executable/static keys are not needed in the semantic state snapshot.
        all.retain(|k| *k != c.program && *k != c.named["event_authority"]);
        if let Ok(table) = std::env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
            all.push(key(&table)?);
            all.sort();
            all.dedup();
        }
        let snapshot = Snapshot::fetch(self.client, &all, third.slot).map_err(|e| e.to_string())?;
        let c = Context::resolve(
            self.layout,
            &snapshot,
            position,
            self.signer.public_key(),
            kind,
            side,
        )?;
        Ok((c, snapshot))
    }
    fn message(
        &self,
        c: &Context,
        s: &Snapshot,
        ixs: &[Instruction],
        hash: [u8; 32],
    ) -> Result<Vec<u8>> {
        Self::compile_message_for(c.buyer, s, ixs, hash)
    }
    pub(crate) fn compile_message_for(
        payer: [u8; 32],
        s: &Snapshot,
        ixs: &[Instruction],
        hash: [u8; 32],
    ) -> Result<Vec<u8>> {
        if let Ok(table) = std::env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
            let table = key(&table)?;
            let a = s.account(table)?;
            if a.owner != "AddressLookupTab1e1111111111111111111111111"
                || a.data.len() < 56
                || (a.data.len() - 56) % 32 != 0
                || a.data[..4] != 1u32.to_le_bytes()
            {
                return Err("invalid liquidation lookup table".into());
            }
            let deactivated = u64::from_le_bytes(a.data[4..12].try_into().unwrap());
            let extended = u64::from_le_bytes(a.data[12..20].try_into().unwrap());
            if deactivated != u64::MAX || extended >= s.slot {
                return Err("lookup table deactivated or not warmed up".into());
            }
            let addresses = a.data[56..]
                .chunks_exact(32)
                .map(|v| v.try_into().unwrap())
                .collect::<Vec<_>>();
            return crate::transaction::compile_v0_message(payer, ixs, hash, table, &addresses)
                .map_err(|e| e.to_string());
        }
        compile_message(payer, ixs, hash).map_err(|e| e.to_string())
    }
    fn simulate(
        &self,
        c: &Context,
        snapshot: &Snapshot,
        ixs: &[Instruction],
        post: &[[u8; 32]],
    ) -> Result<serde_json::Value> {
        self.simulate_for(c.buyer, snapshot, ixs, post)
    }
    fn simulate_for(
        &self,
        payer: [u8; 32],
        snapshot: &Snapshot,
        ixs: &[Instruction],
        post: &[[u8; 32]],
    ) -> Result<serde_json::Value> {
        let message = Self::compile_message_for(payer, snapshot, ixs, [0; 32])?;
        let wire = serialize_transaction(&[[0; 64]], &message);
        if wire.len() > 1232 {
            return Err(format!(
                "transaction is {} bytes; configure a liquidation lookup table",
                wire.len()
            ));
        }
        self.client.call("simulateTransaction",serde_json::json!([base64(&wire),{"encoding":"base64","commitment":"confirmed","minContextSlot":snapshot.slot,"sigVerify":false,"replaceRecentBlockhash":true,"accounts":{"encoding":"base64","addresses":post.iter().copied().map(address).collect::<Vec<_>>()}}])).map_err(|e|e.to_string())
    }
    fn preview<T: BorshDeserialize>(
        &self,
        c: &Context,
        s: &Snapshot,
        ix: Instruction,
    ) -> Result<Option<T>> {
        let mut instructions = flash::resources()?;
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
        flash::decode_return(&value["value"]["returnData"], c.program).map(Some)
    }
    fn attempt(
        &self,
        position: [u8; 32],
        kind: &str,
        side: u8,
    ) -> Result<Option<(Option<String>, String)>> {
        let (c, s) = self.fresh(position, kind, side)?;
        if c.shares == 0 {
            return Ok(None);
        }
        let mut preview = flash::resources()?;
        preview.push(c.health_preview(self.contract)?);
        let result = self.simulate(&c, &s, &preview, &[])?;
        if !result["value"]["err"].is_null() {
            return Err(format!(
                "health preview rejected: {}",
                result["value"]["err"]
            ));
        }
        let health: LiquidationHealth =
            flash::decode_return(&result["value"]["returnData"], c.program)?;
        if !health.eligible() {
            return Ok(None);
        }
        // This profile monitors eligibility without signing a registration.
        if self.profile == "lending-trigger" {
            return Ok(Some((
                None,
                "eligible health preview; atomic execution belongs to bidder/settler".into(),
            )));
        }
        let inventory = self.inventory_mode()? && self.profile != "lending-settler";
        if inventory
            && std::env::var("KEEPER_FLASH_MARKET").ok().as_deref()
                != Some(&address(c.market.address))
        {
            return Ok(None);
        }
        let min_profit = if inventory {
            0
        } else {
            required_u64("KEEPER_LIQUIDATION_MIN_PROFIT")?
        };
        let setup = c.setup(&s)?;
        if !setup.is_empty() {
            return self
                .submit(&c, &s, "prepare_accounts", setup, 0, false)
                .map(Some);
        }
        if self.profile != "lending-settler" {
            let cap = required_u64("KEEPER_FLASH_MAX_REPAY")?;
            if cap == 0 {
                return Err("KEEPER_FLASH_MAX_REPAY must be positive".into());
            }
            let partial = search_profitable(cap.min(health.debt), min_profit, |repay| {
                let Some(q) = self.preview::<FlashLiquidationQuote>(
                    &c,
                    &s,
                    c.preview_flash(self.contract, repay, false)?,
                )?
                else {
                    return Ok(None);
                };
                // Measure the route without the final profit floor; otherwise a
                // below-floor probe rejects before exposing the score needed to
                // refine the search. The selected transaction is rebuilt below
                // with the configured floor enforced on chain.
                let Some(ixs) = self.flash_plan(&c, &s, &q, repay, 0)? else {
                    return Ok(None);
                };
                Ok(self
                    .liquidation_profit(&c, &s, &ixs, q.full)?
                    .map(|profit| (profit, (repay, q))))
            })?;
            if let Some((repay, quote)) = partial {
                let ixs = self
                    .flash_plan(&c, &s, &quote, repay, min_profit)?
                    .ok_or("selected flash route no longer meets configured bounds")?;
                if !self.profitable(&c, &s, &ixs, min_profit, false)? {
                    return Err("selected flash route failed protected revalidation".into());
                }
                return self
                    .submit(&c, &s, "flash", ixs, min_profit, true)
                    .map(Some);
            }
            if cap >= health.debt
                && let Some(q) = self.preview::<FlashLiquidationQuote>(
                    &c,
                    &s,
                    c.preview_flash(self.contract, cap, true)?,
                )?
                && let Some(ixs) = self.flash_plan(&c, &s, &q, cap, min_profit)?
                && self.profitable(&c, &s, &ixs, min_profit, q.full)?
            {
                return self
                    .submit(&c, &s, "flash_full", ixs, min_profit, true)
                    .map(Some);
            }
        }
        if self.profile == "lending-settler" || self.profile == "leverage" {
            let min_profit = required_u64("KEEPER_LIQUIDATION_MIN_PROFIT")?;
            let partial =
                search_profitable(c.collateral.saturating_sub(1), min_profit, |amount| {
                    let Some(q) = self.preview::<EmergencyLiquidationQuote>(
                        &c,
                        &s,
                        c.preview_emergency(self.contract, amount, false)?,
                    )?
                    else {
                        return Ok(None);
                    };
                    let mut ixs = flash::resources()?;
                    ixs.push(c.emergency(self.contract, &q)?);
                    Ok(self
                        .liquidation_profit(&c, &s, &ixs, false)?
                        .map(|profit| (profit, ixs)))
                })?;
            if let Some(ixs) = partial {
                return self
                    .submit(&c, &s, "emergency", ixs, min_profit, true)
                    .map(Some);
            }
            // Permission for full closure is determined by the program's preview;
            // failures of the keeper's partial probes are never proof of permission.
            if let Some(q) = self.preview::<EmergencyLiquidationQuote>(
                &c,
                &s,
                c.preview_emergency(self.contract, c.collateral, true)?,
            )? {
                let mut ixs = flash::resources()?;
                ixs.push(c.emergency(self.contract, &q)?);
                if self.profitable(&c, &s, &ixs, min_profit, true)? {
                    return self
                        .submit(&c, &s, "emergency_full", ixs, min_profit, true)
                        .map(Some);
                }
            }
        }
        Ok(Some((None, "eligible but unfilled: no adaptively searched permitted route met the configured payment/profit bounds".into())))
    }
    fn inventory_mode(&self) -> Result<bool> {
        match std::env::var("KEEPER_FLASH_SETTLEMENT")
            .unwrap_or_else(|_| "dusk".into())
            .as_str()
        {
            "dusk" => Ok(false),
            "inventory" => Ok(true),
            _ => Err("KEEPER_FLASH_SETTLEMENT must be dusk or inventory".into()),
        }
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
            let (payment, gross) = flash::inventory_payment(c, s, q.payment)?;
            if gross > required_u64("KEEPER_FLASH_MAX_PAYMENT_DEBIT")?
                || q.collateral_credit < required_u64("KEEPER_FLASH_MIN_COLLATERAL_CREDIT")?
            {
                return Ok(None);
            }
            c.flash(
                self.contract,
                q,
                flash::FlashPlanOptions {
                    max_repayment: repayment,
                    prefix: flash::resources()?,
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
                flash::FlashPlanOptions {
                    max_repayment: repayment,
                    prefix: flash::resources()?,
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
        let post = [
            c.position,
            c.named["buyer_refund_account"],
            c.named["buyer_collateral_account"],
            c.named["session"],
            c.named["repayment_vault"],
        ];
        let value = self.simulate(c, s, ixs, &post)?;
        if !value["value"]["err"].is_null() {
            if compute_limit_rejection(&value) {
                eprintln!(
                    "keeper liquidation execution exceeded transaction compute limit; trying other permitted sizes"
                );
                return Ok(None);
            }
            return if expected_bound_rejection(&value) {
                Ok(None)
            } else {
                Err(format!(
                    "unexpected execution rejection: {}",
                    value["value"]
                ))
            };
        }
        if post_bytes(&value, 3)?.is_some() {
            return Err("flash session did not close".into());
        }
        let payment_before = s
            .accounts
            .get(&address(post[4]))
            .and_then(Option::as_ref)
            .and_then(|a| flash::token_amount(&a.data))
            .unwrap_or(0);
        let payment_after = post_bytes(&value, 4)?
            .as_deref()
            .and_then(flash::token_amount)
            .unwrap_or(0);
        if payment_after != payment_before {
            return Err("isolated payment vault did not return to its prior balance".into());
        }
        let debt_before =
            flash::token_amount(&s.account(post[1])?.data).ok_or("debt token balance")?;
        let debt_after =
            flash::token_amount(&post_bytes(&value, 1)?.ok_or("missing debt token post-state")?)
                .ok_or("debt token post balance")?;
        let collateral_before =
            flash::token_amount(&s.account(post[2])?.data).ok_or("collateral balance")?;
        let collateral_after =
            flash::token_amount(&post_bytes(&value, 2)?.ok_or("missing collateral post-state")?)
                .ok_or("collateral post balance")?;
        let is_flash = ixs.iter().any(|i| {
            i.data.get(..8)
                == Some(sha2::Sha256::digest(b"global:begin_flash_liquidation")[..8].as_ref())
        });
        if self.inventory_mode()? && is_flash {
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
        let profit = if self.inventory_mode()? && is_flash {
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
        liquidation: bool,
    ) -> Result<(Option<String>, String)> {
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
        let (fresh, now) = self.fresh(c.position, &c.kind, c.debt_asset)?;
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
        let signature = journal.submit(
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
        )?;
        Ok((
            signature,
            format!("{action}: journaled; pending signatures are reconciled before replacement"),
        ))
    }
}

fn recovered_submission_status(outcome: &str) -> (OutcomeStatus, ReasonCode) {
    match outcome {
        "executed" => (OutcomeStatus::Executed, ReasonCode::Confirmed),
        "failed" => (
            OutcomeStatus::RetryableFailure,
            ReasonCode::SimulationRejected,
        ),
        "expired_absent" | "abandoned_before_submission" => {
            (OutcomeStatus::Skipped, ReasonCode::BlockhashExpired)
        }
        _ => (
            OutcomeStatus::RetryableFailure,
            ReasonCode::ConfirmationUnknown,
        ),
    }
}
pub fn slices(cap: u64) -> Vec<u64> {
    let mut out = vec![];
    let mut value = cap;
    while value > 0 && out.len() < 16 {
        out.push(value);
        value /= 2;
    }
    if cap > 0 && out.last() != Some(&1) {
        out.push(1);
    }
    out
}

const PROFIT_SEARCH_GRID_SEGMENTS: u64 = 8;
const PROFIT_SEARCH_LOCAL_WIDTH: u64 = 64;
const PROFIT_SEARCH_MAX_PROBES: usize = 256;

fn probe_profit<T, F>(
    amount: u64,
    minimum: u64,
    probes: &mut usize,
    visited: &mut BTreeSet<u64>,
    scores: &mut BTreeMap<u64, u64>,
    evaluate: &mut F,
) -> Result<Option<T>>
where
    F: FnMut(u64) -> Result<Option<(u64, T)>>,
{
    if amount == 0 || *probes >= PROFIT_SEARCH_MAX_PROBES || !visited.insert(amount) {
        return Ok(None);
    }
    *probes += 1;
    let Some((score, payload)) = evaluate(amount)? else {
        return Ok(None);
    };
    scores.insert(amount, score);
    Ok((score >= minimum).then_some(payload))
}

/// Start with the old geometric probes, then use their measured execution
/// profit to refine the best interval. Once the interval is small, scan every
/// integer amount so rounding peaks are not hidden between coarse samples.
fn search_profitable<T, F>(cap: u64, minimum: u64, mut evaluate: F) -> Result<Option<T>>
where
    F: FnMut(u64) -> Result<Option<(u64, T)>>,
{
    if cap == 0 {
        return Ok(None);
    }
    let mut probes = 0usize;
    let mut visited = BTreeSet::new();
    let mut scores = BTreeMap::new();
    for amount in slices(cap) {
        if let Some(payload) = probe_profit(
            amount,
            minimum,
            &mut probes,
            &mut visited,
            &mut scores,
            &mut evaluate,
        )? {
            return Ok(Some(payload));
        }
    }
    let Some((&initial_best, _)) = scores
        .iter()
        .max_by_key(|(amount, score)| (**score, **amount))
    else {
        return Ok(None);
    };
    let mut low = visited
        .range(..initial_best)
        .next_back()
        .copied()
        .unwrap_or(1);
    let mut high = visited
        .range((Excluded(initial_best), Included(cap)))
        .next()
        .copied()
        .unwrap_or(cap);

    while high.saturating_sub(low) > PROFIT_SEARCH_LOCAL_WIDTH && probes < PROFIT_SEARCH_MAX_PROBES
    {
        let width = high - low;
        for step in 0..=PROFIT_SEARCH_GRID_SEGMENTS {
            let amount = low
                + u64::try_from(
                    u128::from(width) * u128::from(step) / u128::from(PROFIT_SEARCH_GRID_SEGMENTS),
                )
                .map_err(|_| "profit-search amount overflow")?;
            if let Some(payload) = probe_profit(
                amount,
                minimum,
                &mut probes,
                &mut visited,
                &mut scores,
                &mut evaluate,
            )? {
                return Ok(Some(payload));
            }
        }
        let Some((&best, _)) = scores
            .range(low..=high)
            .max_by_key(|(amount, score)| (**score, **amount))
        else {
            break;
        };
        let next_low = visited.range(low..best).next_back().copied().unwrap_or(low);
        let next_high = visited
            .range((Excluded(best), Included(high)))
            .next()
            .copied()
            .unwrap_or(high);
        if (next_low, next_high) == (low, high) {
            break;
        }
        low = next_low;
        high = next_high;
    }

    if high.saturating_sub(low) <= PROFIT_SEARCH_LOCAL_WIDTH {
        for amount in (low..=high).rev() {
            if let Some(payload) = probe_profit(
                amount,
                minimum,
                &mut probes,
                &mut visited,
                &mut scores,
                &mut evaluate,
            )? {
                return Ok(Some(payload));
            }
        }
    }
    Ok(None)
}

fn send_limit_reached(profile: &str, attempted: usize, maximum: usize) -> bool {
    profile != "lending-trigger" && attempted >= maximum
}

fn counts_toward_send_limit(profile: &str, detail: &str) -> bool {
    profile != "lending-trigger" && !detail.starts_with("eligible but unfilled:")
}
fn required_u64(name: &str) -> Result<u64> {
    std::env::var(name)
        .map_err(|_| format!("{name} must be configured in debt-token atoms"))?
        .parse()
        .map_err(|_| format!("invalid {name}"))
}
fn post_bytes(value: &serde_json::Value, index: usize) -> Result<Option<Vec<u8>>> {
    let a = value["value"]["accounts"]
        .as_array()
        .and_then(|v| v.get(index))
        .ok_or("simulation omitted requested post-state")?;
    if a.is_null() {
        return Ok(None);
    }
    if a["data"][1] != "base64" {
        return Err("post-state encoding".into());
    }
    Ok(Some(
        crate::discovery::decode_base64(a["data"][0].as_str().ok_or("post-state data")?)
            .ok_or("post-state base64")?,
    ))
}
fn state_hash(s: &Snapshot, epoch: u64) -> Result<String> {
    let mut accounts = s.accounts.clone();
    accounts.remove(CLOCK);
    Ok(format!(
        "{:x}",
        Sha256::digest(
            [
                serde_json::to_vec(&accounts).map_err(|e| e.to_string())?,
                epoch.to_le_bytes().to_vec()
            ]
            .concat()
        )
    ))
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

// Financial bounds are expected during size probing. ABI, account, invariant,
// and arithmetic failures must remain visible instead of looking unprofitable.
fn expected_bound_rejection(value: &serde_json::Value) -> bool {
    let Some(logs) = value["value"]["logs"].as_array() else {
        return false;
    };
    [
        "PositionNotLiquidatable",
        "LiquidationRepayTooLarge",
        "LiquidationDoesNotImproveHealth",
        "InsufficientAmount",
        "InsufficientDebt",
        "SlippageExceeded",
        "InsufficientCashReserve0",
        "InsufficientCashReserve1",
    ]
    .iter()
    .any(|name| {
        logs.iter().any(|l| {
            l.as_str()
                .is_some_and(|line| line.contains(&format!("Error Code: {name}.")))
        })
    })
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

fn compute_limit_rejection(value: &serde_json::Value) -> bool {
    // RPCs may expose the structured error without the compact enum spelling
    // in logs. A compute-bound large fill must not stop smaller probes.
    if value["value"]["err"]
        .to_string()
        .contains("ComputationalBudgetExceeded")
    {
        return true;
    }
    value["value"]["logs"].as_array().is_some_and(|logs| {
        logs.iter().any(|v| {
            v.as_str().is_some_and(|line| {
                line.contains("exceeded CUs meter")
                    || line.contains("ComputationalBudgetExceeded")
                    || line.ends_with("failed: Computational budget exceeded")
            })
        })
    })
}

// The pinned program uses this error for `require_gt!(shares, 0)` as well
// as invalid index arithmetic. Recognize only the explicit zero/zero bound;
// all other arithmetic failures remain visible and abort the attempt.
fn zero_share_probe(value: &serde_json::Value) -> bool {
    let Some(logs) = value["value"]["logs"].as_array() else {
        return false;
    };
    logs.iter().any(|v| {
        v.as_str()
            .is_some_and(|s| s.contains("Error Code: DebtShareDivisionOverflow."))
    }) && logs.iter().any(|v| v == "Program log: Left: 0")
        && logs.iter().any(|v| v == "Program log: Right: 0")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signing_fingerprint_invalidates_epoch_and_schedule_changes() {
        let mut a = Snapshot {
            slot: 1,
            accounts: Default::default(),
        };
        a.accounts.insert(
            "mint".into(),
            Some(flash::Account {
                owner: "token".into(),
                data: vec![1],
                executable: false,
            }),
        );
        let initial = state_hash(&a, 4).unwrap();
        let mut b = a.clone();
        b.slot = 2;
        b.accounts.insert(
            CLOCK.into(),
            Some(flash::Account {
                owner: "sysvar".into(),
                data: vec![2],
                executable: false,
            }),
        );
        assert_eq!(initial, state_hash(&b, 4).unwrap());
        assert_ne!(initial, state_hash(&b, 5).unwrap());
        b.accounts.get_mut("mint").unwrap().as_mut().unwrap().data[0] = 2;
        assert_ne!(initial, state_hash(&b, 4).unwrap());
    }
    #[test]
    fn compute_limits_allow_smaller_probes_for_all_rpc_error_forms() {
        for value in [
            serde_json::json!({"value":{"err":{"InstructionError":[4,"ComputationalBudgetExceeded"]},"logs":[]}}),
            serde_json::json!({"value":{"err":"TransactionErrorInstructionError { index: 4, error: ComputationalBudgetExceeded }","logs":[]}}),
            serde_json::json!({"value":{"err":"execution failed","logs":["Program dusk failed: Computational budget exceeded"]}}),
            serde_json::json!({"value":{"err":"execution failed","logs":["exceeded CUs meter at BPF instruction"]}}),
        ] {
            assert!(compute_limit_rejection(&value));
        }
        assert!(!compute_limit_rejection(
            &serde_json::json!({"value":{"err":{"InstructionError":[4,{"Custom":6001}]},"logs":["Error Code: BrokenInvariant."]}})
        ));
    }

    #[test]
    fn adaptive_profit_search_finds_non_geometric_rounding_peak() {
        let peaks = [906, 924, 981, 986, 991, 996];
        let mut visited = BTreeSet::new();
        let selected = search_profitable(1_000, 8, |amount| {
            assert!(visited.insert(amount), "amount {amount} probed twice");
            let profit = if peaks.contains(&amount) {
                8
            } else if amount >= 900 {
                7
            } else if amount >= 500 {
                6
            } else if amount >= 250 {
                3
            } else {
                2
            };
            Ok(Some((profit, amount)))
        })
        .unwrap();
        assert_eq!(selected, Some(996));
        assert!(!slices(1_000).contains(&996));
        assert!(visited.len() < PROFIT_SEARCH_MAX_PROBES);
    }

    #[test]
    fn adaptive_profit_search_handles_empty_and_unavailable_domains() {
        let mut calls = 0;
        assert_eq!(
            search_profitable::<u64, _>(0, 1, |_| {
                calls += 1;
                Ok(Some((1, 1)))
            })
            .unwrap(),
            None
        );
        assert_eq!(calls, 0);
        assert_eq!(
            search_profitable::<u64, _>(u64::MAX, 1, |_| {
                calls += 1;
                Ok(None)
            })
            .unwrap(),
            None
        );
        assert!(calls <= 17);
    }

    #[test]
    fn health_monitor_does_not_consume_transaction_send_budget() {
        assert!(!send_limit_reached("lending-trigger", 4, 4));
        assert!(!counts_toward_send_limit(
            "lending-trigger",
            "eligible health preview; atomic execution belongs to bidder/settler"
        ));
        assert!(send_limit_reached("lending-bidder", 4, 4));
        assert!(counts_toward_send_limit(
            "lending-bidder",
            "shadow flash: simulated; no signature produced"
        ));
        assert!(!counts_toward_send_limit(
            "lending-bidder",
            "eligible but unfilled: no route"
        ));
    }

    #[test]
    fn zero_share_probe_does_not_hide_other_arithmetic_failures() {
        let mut value = serde_json::json!({"value":{"logs":["Error Code: DebtShareDivisionOverflow.","Program log: Left: 0","Program log: Right: 0"]}});
        assert!(zero_share_probe(&value));
        value["value"]["logs"][2] = serde_json::json!("Program log: Right: 1000000000");
        assert!(!zero_share_probe(&value));
        assert!(!expected_bound_rejection(&value));
    }

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

    #[test]
    fn finalized_failure_is_not_reported_as_expired() {
        assert_eq!(
            recovered_submission_status("failed"),
            (
                OutcomeStatus::RetryableFailure,
                ReasonCode::SimulationRejected
            )
        );
        assert_eq!(
            recovered_submission_status("expired_absent"),
            (OutcomeStatus::Skipped, ReasonCode::BlockhashExpired)
        );
    }
}
