//! Conditional orders use the delegate program's own trigger and settlement
//! checks. This module assembles accounts; it never invents a trigger price.
use crate::{
    accounts::{ASSOCIATED_TOKEN_PROGRAM, MarketAccounts, associated_token_account},
    discovery::account_discriminator,
    flash::{Snapshot, address, key},
    transaction::{AccountMeta, Instruction},
};
use dusk_adapter::{
    AccountLayoutManifest, DelegatedCloseLeverageArgs, DelegatedCpiArgs, ExecuteEntryOrderArgs,
    ExecuteOrderArgs, InstructionContract, KeeperInstructionArguments as Args,
    derive_program_address, encode_keeper_instruction,
};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, String>;

#[derive(serde::Serialize)]
pub struct OrderPlan {
    pub instructions: Vec<Instruction>,
    pub named: BTreeMap<String, [u8; 32]>,
    pub order: [u8; 32],
    pub market: [u8; 32],
    pub conflict: [u8; 32],
    pub side: u8,
    pub kind: String,
    pub reward_account: [u8; 32],
}

fn pda(program: [u8; 32], seeds: &[&[u8]]) -> Result<[u8; 32]> {
    key(&derive_program_address(
        &seeds.iter().map(|s| s.to_vec()).collect::<Vec<_>>(),
        &address(program),
    )
    .map_err(|e| e.to_string())?
    .address)
}

fn instruction(
    contract: &InstructionContract,
    args: Args,
    named: &BTreeMap<String, [u8; 32]>,
) -> Result<Instruction> {
    let spec = contract
        .instructions
        .iter()
        .find(|s| s.key == args.specification_key())
        .ok_or("missing order instruction")?;
    let program = key(&spec.program_id)?;
    let mut accounts = Vec::new();
    for a in &spec.accounts {
        let resolved = named.get(&a.name).copied().or_else(|| {
            a.fixed_address
                .as_deref()
                .and_then(crate::accounts::decode_key)
        });
        let absent = resolved.is_none() && a.optional;
        let pubkey = resolved
            .or(absent.then_some(program))
            .ok_or_else(|| format!("missing order account {}", a.name))?;
        accounts.push(AccountMeta {
            pubkey,
            is_signer: !absent && a.signer,
            is_writable: !absent && a.writable,
        });
    }
    Ok(Instruction {
        program_id: program,
        accounts,
        data: encode_keeper_instruction(contract, &args).map_err(|e| e.to_string())?,
    })
}

pub fn build_order_plan(
    contract: &InstructionContract,
    layout: &AccountLayoutManifest,
    orders: &AccountLayoutManifest,
    snapshot: &Snapshot,
    order: [u8; 32],
    kind: &str,
    executor: [u8; 32],
) -> Result<OrderPlan> {
    orders
        .assert_matches(&contract.protocol_revision)
        .map_err(|e| e.to_string())?;
    layout
        .assert_matches(&contract.protocol_revision)
        .map_err(|e| e.to_string())?;
    let (program, delegate) = (key(&layout.program_id)?, key(&orders.program_id)?);
    if !contract
        .programs
        .iter()
        .any(|p| p.program_id == orders.program_id && p.idl_sha256 == orders.idl_sha256)
    {
        return Err("order layout differs from the pinned delegate".into());
    }
    let raw = snapshot.owned(order, delegate)?;
    if raw.get(..8) != Some(account_discriminator(kind).as_slice()) {
        return Err("order discriminator".into());
    }
    let reader = orders.reader(kind).map_err(|e| e.to_string())?;
    let read = |name: &str| reader.pubkey(name, raw).map_err(|e| e.to_string());
    let (owner, market_key) = (read("owner")?, read("market")?);
    let market_raw = snapshot.owned(market_key, program)?;
    let market =
        MarketAccounts::decode(layout, market_key, market_raw).ok_or("order market layout")?;
    let mr = layout.reader("Market").map_err(|e| e.to_string())?;
    let order_id = reader.u64("order_id", raw).map_err(|e| e.to_string())?;
    let futarchy = pda(program, &[b"futarchy_authority"])?;
    let treasury = layout
        .reader("FutarchyAuthority")
        .map_err(|e| e.to_string())?
        .pubkey(
            "recipients.futarchy_treasury",
            snapshot.owned(futarchy, program)?,
        )
        .map_err(|e| e.to_string())?;
    let mut named = BTreeMap::from([
        ("order".into(), order),
        ("market".into(), market_key),
        ("owner".into(), owner),
        ("order_owner".into(), owner),
        ("position_owner".into(), owner),
        ("executor".into(), executor),
        ("authority".into(), executor),
        ("futarchy_authority".into(), futarchy),
        ("base_mint".into(), market.base_mint),
        ("quote_mint".into(), market.quote_mint),
        ("base_reserve_vault".into(), market.base_reserve_vault),
        ("quote_reserve_vault".into(), market.quote_reserve_vault),
        ("ylp_mint".into(), market.ylp_mint),
        ("dusk_program".into(), program),
        ("program".into(), program),
        (
            "event_authority".into(),
            pda(program, &[b"__event_authority"])?,
        ),
        (
            "dusk_event_authority".into(),
            pda(program, &[b"__event_authority"])?,
        ),
    ]);
    let mut setup = crate::flash::resources()?;
    let mut ata = |who: [u8; 32], mint: [u8; 32]| -> Result<[u8; 32]> {
        let mint_account = snapshot.account(mint)?;
        dusk_adapter::transfer_fees::decode_mint_fees(&mint_account.owner, &mint_account.data)?;
        let token = key(&mint_account.owner)?;
        let a = associated_token_account(who, token, mint).ok_or("order ATA")?;
        if snapshot
            .accounts
            .get(&address(a))
            .and_then(Option::as_ref)
            .is_none_or(|account| account.owner == address([0; 32]) && account.data.is_empty())
        {
            setup.push(Instruction {
                program_id: key(ASSOCIATED_TOKEN_PROGRAM)?,
                data: vec![1],
                accounts: vec![
                    AccountMeta::signer(executor),
                    AccountMeta::writable(a),
                    AccountMeta::readonly(who),
                    AccountMeta::readonly(mint),
                    AccountMeta::readonly([0; 32]),
                    AccountMeta::readonly(token),
                ],
            });
        } else {
            let account = snapshot.account(a)?;
            if account.owner != mint_account.owner
                || account.data.get(..32) != Some(mint.as_slice())
                || account.data.get(32..64) != Some(who.as_slice())
                || account.data.get(108) != Some(&1)
            {
                return Err("order ATA identity or state".into());
            }
        }
        Ok(a)
    };
    let (side, conflict, reward, mut primary) = if kind == "HlpOrder" {
        if reader.u8("status", raw).map_err(|e| e.to_string())? != 0 {
            return Err("order is no longer open".into());
        }
        let target = read("target_hlp_mint")?;
        let base_hlp = mr
            .pubkey("base_side.hlp_mint", market_raw)
            .map_err(|e| e.to_string())?;
        let quote_hlp = mr
            .pubkey("quote_side.hlp_mint", market_raw)
            .map_err(|e| e.to_string())?;
        let side = if target == base_hlp {
            0
        } else if target == quote_hlp {
            1
        } else {
            return Err("retired hLP order needs owner cancellation/yield settlement".into());
        };
        if pda(
            delegate,
            &[
                b"hlp_order",
                &market_key,
                &owner,
                &target,
                &order_id.to_le_bytes(),
            ],
        )? != order
        {
            return Err("hLP order PDA".into());
        }
        let mint = if side == 0 {
            market.base_mint
        } else {
            market.quote_mint
        };
        let reward = ata(executor, mint)?;
        named.extend([
            ("target_hlp_mint".into(), target),
            ("custody_hlp_account".into(), read("custody_hlp_account")?),
            ("custody_target_account".into(), ata(order, mint)?),
            ("owner_target_account".into(), ata(owner, mint)?),
            ("executor_target_account".into(), reward),
            ("fee_recipient".into(), ata(treasury, mint)?),
            (
                "hlp_ylp_account".into(),
                if side == 0 {
                    market.base_ylp_vault
                } else {
                    market.quote_ylp_vault
                },
            ),
            (
                "borrowed_interest_vault".into(),
                if side == 0 {
                    market.quote_interest_vault
                } else {
                    market.base_interest_vault
                },
            ),
            (
                "base_yield_account".into(),
                pda(
                    program,
                    &[
                        b"yield",
                        &market_key,
                        &order,
                        &target,
                        &market.base_mint,
                        &[1],
                    ],
                )?,
            ),
            (
                "quote_yield_account".into(),
                pda(
                    program,
                    &[
                        b"yield",
                        &market_key,
                        &order,
                        &target,
                        &market.quote_mint,
                        &[1],
                    ],
                )?,
            ),
        ]);
        (
            side,
            market_key,
            reward,
            instruction(
                contract,
                Args::ExecuteHlpOrder(ExecuteOrderArgs { order_id }),
                &named,
            )?,
        )
    } else {
        let position = read("position")?;
        let position_reader = layout
            .reader("LeveragePosition")
            .map_err(|e| e.to_string())?;
        let side = if kind == "LeverageEntryOrder" {
            reader.u8("debt_asset", raw).map_err(|e| e.to_string())?
        } else {
            position_reader
                .u8("debt_asset", snapshot.owned(position, program)?)
                .map_err(|e| e.to_string())?
        };
        if side > 1 {
            return Err("order debt side".into());
        }
        let sides = market.oriented(side == 0);
        let (reward, owner_ata, fee) = (
            ata(executor, sides.debt_mint)?,
            ata(owner, sides.debt_mint)?,
            ata(treasury, sides.debt_mint)?,
        );
        named.extend([
            ("leverage_position".into(), position),
            ("debt_mint".into(), sides.debt_mint),
            ("collateral_mint".into(), sides.collateral_mint),
            ("debt_reserve_vault".into(), sides.debt_reserve_vault),
            (
                "collateral_reserve_vault".into(),
                sides.collateral_reserve_vault,
            ),
            ("debt_interest_vault".into(), sides.debt_interest_vault),
            (
                "leverage_collateral_vault".into(),
                pda(
                    program,
                    &[b"leverage_collateral", &market_key, &sides.collateral_mint],
                )?,
            ),
            ("owner_debt_account".into(), owner_ata),
            ("owner_token_account".into(), owner_ata),
            ("owner_refund_account".into(), owner_ata),
            ("executor_token_account".into(), reward),
            ("executor_bounty_account".into(), reward),
            ("delegate_executor_account".into(), reward),
            ("fee_recipient".into(), fee),
            ("delegate_fee_recipient".into(), fee),
            ("token_mint".into(), sides.debt_mint),
        ]);
        let primary = if kind == "LeverageEntryOrder" {
            if pda(
                delegate,
                &[
                    b"leverage_entry_order",
                    &market_key,
                    &owner,
                    &order_id.to_le_bytes(),
                ],
            )? != order
            {
                return Err("entry order PDA".into());
            }
            if read("debt_mint")? != sides.debt_mint
                || read("collateral_mint")? != sides.collateral_mint
            {
                return Err("entry order mints".into());
            }
            let token = key(&snapshot.account(sides.debt_mint)?.owner)?;
            named.insert(
                "funding_vault".into(),
                associated_token_account(order, token, sides.debt_mint)
                    .ok_or("entry funding ATA")?,
            );
            // The Borsh option follows the last fixed field. Its location is
            // generated from the IDL; no later variable-layout field is read.
            let field = orders
                .accounts
                .iter()
                .find(|a| a.name == kind)
                .and_then(|a| a.fields.iter().find(|f| f.path == "expiry_unix_timestamp"))
                .ok_or("entry option offset")?;
            let offset = field.offset + field.size;
            match raw.get(offset) {
                Some(0) => {}
                Some(1) => {
                    let referrer: [u8; 32] = raw
                        .get(offset + 1..offset + 33)
                        .and_then(|v| v.try_into().ok())
                        .ok_or("entry referrer")?;
                    let partner = pda(program, &[b"referral_partner", &referrer])?;
                    named.insert("referral_partner".into(), partner);
                    named.insert(
                        "referral_accrual".into(),
                        pda(
                            program,
                            &[b"referral_accrual", &partner, &market_key, &sides.debt_mint],
                        )?,
                    );
                }
                _ => return Err("entry referrer option".into()),
            }
            instruction(
                contract,
                Args::ExecuteEntryOrder(ExecuteEntryOrderArgs {
                    order_id,
                    min_owner_refund_out: 0,
                }),
                &named,
            )?
        } else if kind == "LeverageOrder" {
            let position_raw = snapshot.owned(position, program)?;
            if position_reader
                .pubkey("owner", position_raw)
                .map_err(|e| e.to_string())?
                != owner
                || position_reader
                    .pubkey("market", position_raw)
                    .map_err(|e| e.to_string())?
                    != market_key
            {
                return Err("exit order position scope".into());
            }
            if pda(
                delegate,
                &[
                    b"leverage_order",
                    &position,
                    &owner,
                    &order_id.to_le_bytes(),
                ],
            )? != order
            {
                return Err("exit order PDA".into());
            }
            named.insert(
                "leverage_delegation".into(),
                pda(program, &[b"leverage_delegation_v2", &position])?,
            );
            named.insert("delegated_program".into(), delegate);
            let partner = position_reader
                .pubkey("referral_partner", position_raw)
                .map_err(|e| e.to_string())?;
            if partner != [0; 32] {
                named.insert("referral_partner".into(), partner);
                named.insert(
                    "referral_accrual".into(),
                    pda(
                        program,
                        &[b"referral_accrual", &partner, &market_key, &sides.debt_mint],
                    )?,
                );
            }
            let kind = reader.u8("kind", raw).map_err(|e| e.to_string())?;
            let before = instruction(
                contract,
                match kind {
                    1 => Args::BeforeTakeProfit(ExecuteOrderArgs { order_id }),
                    2 => Args::BeforeStopLoss(ExecuteOrderArgs { order_id }),
                    _ => return Err("exit order kind".into()),
                },
                &named,
            )?;
            let after = instruction(
                contract,
                Args::AfterCloseOrder(ExecuteOrderArgs { order_id }),
                &named,
            )?;
            let close_bps = u16::from_le_bytes(
                reader
                    .bytes("close_bps", raw)
                    .map_err(|e| e.to_string())?
                    .try_into()
                    .map_err(|_| "close fraction")?,
            );
            let mut primary = instruction(
                contract,
                Args::DelegatedCloseLeverage(DelegatedCloseLeverageArgs {
                    debt_asset: side,
                    min_amount_out: 0,
                    close_bps,
                    delegated: DelegatedCpiArgs {
                        before_ix_data: before.data,
                        after_ix_data: after.data,
                        before_accounts_len: before.accounts.len() as u16,
                    },
                }),
                &named,
            )?;
            primary.accounts.extend(market.hlp_remaining_accounts());
            primary.accounts.extend(before.accounts);
            primary.accounts.extend(after.accounts);
            primary
        } else {
            return Err("unsupported order kind".into());
        };
        (side, position, reward, primary)
    };
    if kind != "LeverageOrder" {
        primary.accounts.extend(market.hlp_remaining_accounts());
    }
    setup.push(primary);
    Ok(OrderPlan {
        instructions: setup,
        named,
        order,
        market: market_key,
        conflict,
        side,
        kind: kind.into(),
        reward_account: reward,
    })
}

fn order_layout() -> Result<AccountLayoutManifest> {
    let path = std::env::var("DUSK_ORDER_LAYOUT")
        .unwrap_or_else(|_| "protocol/keeper-order-layout.v1.json".into());
    AccountLayoutManifest::from_json(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn run_plan_cli() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    let snapshot: Snapshot = serde_json::from_value(value["snapshot"].clone())?;
    let layout = AccountLayoutManifest::from_json(&std::fs::read_to_string(
        "protocol/keeper-account-layout.v1.json",
    )?)?;
    let contract = InstructionContract::from_json(&std::fs::read_to_string(
        "protocol/keeper-instructions.v1.json",
    )?)?;
    let plan = build_order_plan(
        &contract,
        &layout,
        &order_layout()?,
        &snapshot,
        key(value["order"].as_str().ok_or("order")?)?,
        value["kind"].as_str().ok_or("kind")?,
        key(value["executor"].as_str().ok_or("executor")?)?,
    )?;
    println!("{}", serde_json::to_string(&plan)?);
    Ok(())
}

pub struct OrderJob<'a> {
    pub client: &'a crate::discovery::RpcClient,
    pub layout: &'a AccountLayoutManifest,
    pub contract: &'a InstructionContract,
    pub signer: &'a dyn crate::signer::TransactionSigner,
    pub dry_run: bool,
    pub max_sends: usize,
}

fn balance(data: Option<&[u8]>) -> Result<u64> {
    match data {
        None => Ok(0),
        Some([]) => Ok(0),
        Some(raw) => raw
            .get(64..72)
            .and_then(|v| v.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or("order reward token account".into()),
    }
}

impl OrderJob<'_> {
    pub fn run_pass(
        &self,
    ) -> std::result::Result<Vec<crate::execute::AttemptReport>, crate::execute::ExecutionError>
    {
        use crate::execute::ExecutionError;
        let orders = order_layout().map_err(ExecutionError::Assembly)?;
        let markets = self
            .client
            .program_accounts(&self.layout.program_id, account_discriminator("Market"))?;
        let active: std::collections::BTreeSet<_> = markets
            .into_iter()
            .filter_map(|(address, data)| {
                MarketAccounts::decode(self.layout, address, &data).map(|_| address)
            })
            .collect();
        let selected = std::env::var("KEEPER_ORDER_MARKET")
            .ok()
            .map(|v| key(&v))
            .transpose()
            .map_err(ExecutionError::Assembly)?;
        let mut reports = Vec::new();
        let mut attempted = 0;
        for kind in ["LeverageOrder", "HlpOrder", "LeverageEntryOrder"] {
            for (order, raw) in self
                .client
                .program_accounts(&orders.program_id, account_discriminator(kind))?
            {
                let market: [u8; 32] = raw
                    .get(40..72)
                    .and_then(|v| v.try_into().ok())
                    .ok_or_else(|| ExecutionError::Assembly("order identity prefix".into()))?;
                if !active.contains(&market) || selected.is_some_and(|s| s != market) {
                    continue;
                }
                if attempted >= self.max_sends {
                    return Ok(reports);
                }
                let (report, eligible) = self
                    .attempt(&orders, order, market, kind)
                    .map_err(ExecutionError::Assembly)?;
                attempted += usize::from(eligible);
                reports.push(report);
            }
        }
        Ok(reports)
    }

    fn capture(
        &self,
        orders: &AccountLayoutManifest,
        order: [u8; 32],
        market: [u8; 32],
        kind: &str,
    ) -> Result<(OrderPlan, Snapshot)> {
        let program = key(&self.layout.program_id)?;
        let floor = self.client.slot().map_err(|e| e.to_string())?;
        let first =
            Snapshot::fetch(self.client, &[order, market], floor).map_err(|e| e.to_string())?;
        let m = MarketAccounts::decode(self.layout, market, first.owned(market, program)?)
            .ok_or("order market layout")?;
        let mut keys = vec![
            order,
            market,
            m.base_mint,
            m.quote_mint,
            pda(program, &[b"futarchy_authority"])?,
        ];
        if kind == "LeverageOrder" {
            keys.push(
                orders
                    .reader(kind)
                    .map_err(|e| e.to_string())?
                    .pubkey("position", first.owned(order, key(&orders.program_id)?)?)
                    .map_err(|e| e.to_string())?,
            );
        }
        let second = Snapshot::fetch(self.client, &keys, first.slot).map_err(|e| e.to_string())?;
        let draft = build_order_plan(
            self.contract,
            self.layout,
            orders,
            &second,
            order,
            kind,
            self.signer.public_key(),
        )?;
        keys.extend(draft.named.values());
        keys.extend(
            draft
                .instructions
                .iter()
                .flat_map(|ix| ix.accounts.iter().map(|account| account.pubkey)),
        );
        // Reward balances must be observed even when an ATA was not present.
        keys.push(draft.reward_account);
        if let Ok(table) = std::env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
            keys.push(key(&table)?);
        }
        keys.sort();
        keys.dedup();
        let full = Snapshot::fetch(self.client, &keys, second.slot).map_err(|e| e.to_string())?;
        let plan = build_order_plan(
            self.contract,
            self.layout,
            orders,
            &full,
            order,
            kind,
            self.signer.public_key(),
        )?;
        Ok((plan, full))
    }

    fn attempt(
        &self,
        orders: &AccountLayoutManifest,
        order: [u8; 32],
        market: [u8; 32],
        kind: &str,
    ) -> Result<(crate::execute::AttemptReport, bool)> {
        use crate::{
            execute::AttemptReport,
            flash_runtime::FlashJob,
            transaction::{base64, serialize_transaction},
        };
        use keeper_core::{OutcomeStatus, ReasonCode};
        let skipped = |reason, detail| {
            (
                AttemptReport {
                    position: address(order),
                    debt_mint: String::new(),
                    status: OutcomeStatus::Skipped,
                    reason,
                    race: None,
                    signature: None,
                    detail: Some(detail),
                },
                false,
            )
        };
        let (plan, snapshot) = match self.capture(orders, order, market, kind) {
            Ok(value) => value,
            Err(detail)
                if detail == "order is no longer open"
                    || detail.starts_with("retired hLP order") =>
            {
                return Ok(skipped(ReasonCode::AlreadyResolved, detail));
            }
            Err(error) => return Err(error),
        };
        let payout_mint = plan.named.get("debt_mint").copied().unwrap_or_else(|| {
            plan.named[if plan.side == 0 {
                "base_mint"
            } else {
                "quote_mint"
            }]
        });
        let mints = [
            address(plan.named["base_mint"]),
            address(plan.named["quote_mint"]),
        ];
        let fees = crate::fee_notice::read_fees(self.client, &mints, snapshot.slot)
            .map_err(|e| e.to_string())?;
        let unsigned = FlashJob::compile_message_for(
            self.signer.public_key(),
            &snapshot,
            &plan.instructions,
            [0; 32],
        )?;
        let wire = serialize_transaction(&[[0; 64]], &unsigned);
        if wire.len() > 1232 {
            return Ok(skipped(
                ReasonCode::BoundsNotMet,
                "order needs a configured lookup table".into(),
            ));
        }
        let (error, posts) = self
            .client
            .simulate_with_accounts(
                &base64(&wire),
                &[address(order), address(plan.reward_account)],
            )
            .map_err(|e| e.to_string())?;
        if let Some(error) = error {
            return Ok(skipped(
                ReasonCode::BoundsNotMet,
                format!("order trigger/settlement rejected: {error}"),
            ));
        }
        let after_fees = crate::fee_notice::read_fees(self.client, &mints, fees.slot)
            .map_err(|e| e.to_string())?;
        if after_fees.fingerprint != fees.fingerprint {
            return Ok(skipped(
                ReasonCode::StateChanged,
                "order mint fees or epoch changed during evaluation".into(),
            ));
        }
        if posts.len() != 2 {
            return Err("order simulation omitted post-state".into());
        }
        if kind == "HlpOrder" {
            if let Some(raw) = posts[0].as_deref().filter(|raw| !raw.is_empty())
                && orders
                    .reader(kind)
                    .map_err(|e| e.to_string())?
                    .u8("status", raw)
                    .map_err(|e| e.to_string())?
                    != 2
            {
                return Err("hLP order did not reach executed status".into());
            }
        } else if posts[0].as_ref().is_some_and(|v| !v.is_empty()) {
            return Err("executed order account did not close".into());
        }
        let before = balance(
            snapshot
                .accounts
                .get(&address(plan.reward_account))
                .and_then(Option::as_ref)
                .map(|v| v.data.as_slice()),
        )?;
        let after = balance(posts[1].as_deref())?;
        let minimum: u64 = std::env::var("KEEPER_ORDER_MIN_BOUNTY_CREDIT")
            .map_err(|_| "KEEPER_ORDER_MIN_BOUNTY_CREDIT must be configured")?
            .parse()
            .map_err(|_| "invalid order bounty bound")?;
        if after
            .checked_sub(before)
            .is_none_or(|credit| credit < minimum)
        {
            return Ok(skipped(
                ReasonCode::BoundsNotMet,
                "order bounty is below the configured net-credit bound".into(),
            ));
        }
        if self.dry_run {
            let (mut report, _) = skipped(
                ReasonCode::ShadowMode,
                format!("shadow {kind}: native trigger, settlement and bounty checks passed"),
            );
            report.debt_mint = address(payout_mint);
            return Ok((report, true));
        }
        let recheck = Snapshot::fetch(
            self.client,
            &snapshot
                .accounts
                .keys()
                .map(|k| key(k))
                .collect::<Result<Vec<_>>>()?,
            snapshot.slot,
        )
        .map_err(|e| e.to_string())?;
        if serde_json::to_vec(&snapshot.accounts).map_err(|e| e.to_string())?
            != serde_json::to_vec(&recheck.accounts).map_err(|e| e.to_string())?
        {
            return Ok(skipped(
                ReasonCode::StateChanged,
                "order accounts changed after simulation".into(),
            ));
        }
        let block: serde_json::Value = self
            .client
            .call(
                "getLatestBlockhash",
                serde_json::json!([{"commitment":"confirmed"}]),
            )
            .map_err(|e| e.to_string())?;
        let hash = block["value"]["blockhash"]
            .as_str()
            .ok_or("order blockhash")?;
        let height = block["value"]["lastValidBlockHeight"]
            .as_u64()
            .ok_or("order blockheight")?;
        let message = FlashJob::compile_message_for(
            self.signer.public_key(),
            &recheck,
            &plan.instructions,
            key(hash)?,
        )?;
        let signing_fees = crate::fee_notice::read_fees(self.client, &mints, recheck.slot)
            .map_err(|e| e.to_string())?;
        if signing_fees.fingerprint != fees.fingerprint {
            return Ok(skipped(
                ReasonCode::StateChanged,
                "order mint fees or epoch changed before signing".into(),
            ));
        }
        use sha2::Digest;
        let state = bs58::encode(sha2::Sha256::digest(
            serde_json::to_vec(&recheck.accounts).map_err(|e| e.to_string())?,
        ))
        .into_string();
        let mut journal = crate::flash_journal::Journal::connect(
            &std::env::var("DATABASE_URL")
                .map_err(|_| "DATABASE_URL is required before signing")?,
        )?;
        let signature = journal.submit(
            self.client,
            self.signer,
            &self.contract.protocol_revision,
            &address(plan.conflict),
            plan.side,
            &address(plan.market),
            kind,
            recheck.slot,
            &state,
            &message,
            hash,
            height,
        )?;
        Ok((
            AttemptReport {
                position: address(order),
                debt_mint: address(payout_mint),
                status: if signature.is_some() {
                    OutcomeStatus::Executed
                } else {
                    OutcomeStatus::Skipped
                },
                reason: if signature.is_some() {
                    ReasonCode::ConfirmationUnknown
                } else {
                    ReasonCode::StateChanged
                },
                race: None,
                signature,
                detail: Some(format!("submitted {kind}")),
            },
            true,
        ))
    }
}
