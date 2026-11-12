//! Bounded external route search; shares the ordinary Dusk eligibility preview,
//! post-state checks and durable submission machinery with other settlements.
use super::*;
use crate::{
    liquidation::plan::{FlashPlanOptions, inventory_payment, resources, token_amount},
    routing::jupiter::{Client, Config, PROGRAM, Request, ValidatedRoute},
};
use dusk_adapter::transfer_fees::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM, decode_mint_fees};
use std::time::Instant;

pub(super) struct RouteState {
    pub snapshot: Snapshot,
    pub expires_at: Instant,
}
pub(super) fn check_deadline(state: Option<&RouteState>) -> Result<()> {
    if state.is_some_and(|s| Instant::now() >= s.expires_at) {
        return Err("Jupiter route expired; requote on the next pass".into());
    }
    Ok(())
}

pub(super) fn slippage_rejection(value: &serde_json::Value, ixs: &[Instruction]) -> bool {
    let error = &value["value"]["err"]["InstructionError"];
    error[0]
        .as_u64()
        .and_then(|i| ixs.get(i as usize))
        .is_some_and(|ix| address(ix.program_id) == PROGRAM)
        && error[1]["Custom"] == 6001
        && value["value"]["logs"].as_array().is_some_and(|logs| {
            logs.iter().any(|l| {
                l.as_str()
                    .is_some_and(|l| l.contains("Error Code: SlippageToleranceExceeded."))
            })
        })
}

impl LiquidationJob<'_> {
    pub(super) fn jupiter_plan(
        &self,
        c: &Context,
        s: &Snapshot,
        health: &LiquidationHealth,
        pass_budget: &mut usize,
        position_budget: &mut usize,
        full_only: bool,
    ) -> Result<Option<LiquidationPlan>> {
        let config = Config::from_env()?;
        let minimum_profit = required_u64("KEEPER_LIQUIDATION_MIN_PROFIT")?;
        let cap = required_u64("KEEPER_FLASH_MAX_REPAY")?;
        if cap == 0 {
            return Err("KEEPER_FLASH_MAX_REPAY must be positive".into());
        }
        let genesis: String = self
            .client
            .call("getGenesisHash", serde_json::json!([]))
            .map_err(|e| e.to_string())?;
        let local_fork = self.dry_run
            && self.client.is_loopback()
            && std::env::var("KEEPER_JUPITER_LOCAL_FORK").as_deref() == Ok("1");
        if genesis != "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" && !local_fork {
            return Err("Jupiter requires mainnet or an opted-in local shadow fork; configure KEEPER_FLASH_SETTLEMENT=dusk for devnet".into());
        }
        let client = Client::new(&config);
        // HTTP-backed search has its own small budget, not the 256-probe local
        // adaptive search. Reserve a candidate for independently permitted full
        // liquidation; no failure of a partial preview authorizes a full close.
        let full_allowed = cap >= health.debt;
        let partial_count = config.attempts.saturating_sub(usize::from(full_allowed));
        let mut builds = if full_only {
            *position_budget
        } else {
            position_budget.saturating_sub(usize::from(full_allowed))
        };
        let mut candidates: Vec<_> = super::super::search::slices(cap.min(health.debt))
            .into_iter()
            .take(if full_only { 0 } else { partial_count })
            .map(|a| (a, false))
            .collect();
        if full_only && full_allowed {
            candidates.push((cap, true));
        }
        for (repay, full) in candidates {
            if builds == 0 {
                break;
            }
            let Some(q) = self.preview::<FlashLiquidationQuote>(
                c,
                s,
                c.preview_flash(self.contract, repay, full)?,
            )?
            else {
                continue;
            };
            let (payment, gross) = inventory_payment(c, s, q.payment)?;
            let required_output = gross
                .checked_add(minimum_profit)
                .ok_or("Jupiter payment/profit overflow")?;
            let request = request(c, q.collateral_credit);
            let mut max_accounts = config.max_accounts;
            loop {
                if builds == 0 {
                    break;
                }
                builds -= 1;
                *position_budget -= 1;
                let Some(route) = client.build(&config, &request, max_accounts, pass_budget)?
                else {
                    break;
                };
                let mint = s.account(c.named["debt_mint"])?;
                let fee = decode_mint_fees(&mint.owner, &mint.data)?;
                // Conservatively deduct the output transfer fee even if a route
                // already reports net output. Never count withheld tokens as profit.
                let net_minimum = fee.map_or(route.minimum_output, |f| {
                    f.credit(c.epoch, route.minimum_output, false, true)
                });
                if net_minimum < required_output {
                    break;
                }
                let extra = route.read_keys();
                let count = s
                    .accounts
                    .keys()
                    .map(|k| key(k))
                    .collect::<Result<BTreeSet<_>>>()?
                    .union(&extra.iter().copied().collect())
                    .count();
                if count > 100 {
                    if reduce_accounts(&mut max_accounts) {
                        continue;
                    }
                    break;
                }
                // One coherent RPC batch, including route pools, programs and
                // actual lookup-table accounts. Never splice quote API account data.
                let (fresh, snapshot) =
                    self.fresh_with_extra(c.position, &c.kind, c.debt_asset, &extra)?;
                let base = Snapshot {
                    slot: snapshot.slot,
                    accounts: snapshot
                        .accounts
                        .iter()
                        .filter(|(k, _)| s.accounts.contains_key(*k))
                        .map(|(k, a)| (k.clone(), a.clone()))
                        .collect(),
                };
                if state_hash(s, c.epoch)? != state_hash(&base, fresh.epoch)? {
                    return Err("state or fee epoch changed during Jupiter routing; requote".into());
                }
                validate_snapshot(&route, &request, &snapshot)?;
                let expires_at = route.expires_at;
                let instructions = compose(
                    &fresh,
                    self.contract,
                    &q,
                    repay,
                    route,
                    payment.clone(),
                    config.priority_lamports,
                )?;
                let message = self.message(&fresh, &snapshot, &instructions, [0; 32])?;
                if serialize_transaction(&[[0; 64]], &message).len() > 1232 {
                    if reduce_accounts(&mut max_accounts) {
                        continue;
                    }
                    break;
                }
                let state = RouteState {
                    snapshot,
                    expires_at,
                };
                check_deadline(Some(&state))?;
                if self.profitable(
                    &fresh,
                    &state.snapshot,
                    &instructions,
                    minimum_profit,
                    q.full,
                )? {
                    check_deadline(Some(&state))?;
                    return Ok(Some(LiquidationPlan {
                        action: if q.full {
                            "flash_jupiter_full"
                        } else {
                            "flash_jupiter"
                        },
                        instructions,
                        minimum_profit,
                        route_state: Some(state),
                    }));
                }
                // Refresh before full liquidation after an expected partial
                // execution rejection; never recycle the failed route or quote.
                if !full {
                    return Ok(None);
                }
                break;
            }
        }
        Ok(None)
    }
}
fn reduce_accounts(max: &mut u8) -> bool {
    if *max <= 8 {
        return false;
    }
    *max = max.saturating_sub(8).max(8);
    true
}
fn request(c: &Context, amount: u64) -> Request {
    let source = c.named["buyer_collateral_account"];
    let destination = c.named["buyer_refund_account"];
    let mut protected: BTreeSet<_> = c
        .named
        .values()
        .copied()
        .chain([c.position, c.market.address, c.program])
        .collect();
    protected.remove(&source);
    protected.remove(&destination);
    protected.remove(&c.buyer);
    Request {
        buyer: c.buyer,
        input_mint: c.named["collateral_mint"],
        output_mint: c.named["debt_mint"],
        source,
        destination,
        input_program: c.collateral_token,
        output_program: c.debt_token,
        amount,
        protected,
    }
}
fn validate_snapshot(route: &ValidatedRoute, r: &Request, s: &Snapshot) -> Result<()> {
    if !s
        .accounts
        .get(PROGRAM)
        .and_then(Option::as_ref)
        .is_some_and(|a| a.executable)
    {
        return Err("Jupiter program is not deployed on the connected cluster".into());
    }
    for table in &route.tables {
        let account = s.account(*table)?;
        if account.owner != "AddressLookupTab1e1111111111111111111111111" {
            return Err("invalid Jupiter lookup table owner".into());
        }
    }
    for meta in &route.swap.accounts {
        if r.protected.contains(&meta.pubkey)
            && s.accounts
                .get(&address(meta.pubkey))
                .and_then(Option::as_ref)
                .is_some_and(|a| a.executable)
        {
            return Err("Jupiter route may not invoke the protected Dusk program".into());
        }
        if !meta.is_writable || meta.pubkey == r.source || meta.pubkey == r.destination {
            continue;
        }
        if let Some(Some(a)) = s.accounts.get(&address(meta.pubkey))
            && [TOKEN_PROGRAM, TOKEN_2022_PROGRAM].contains(&a.owner.as_str())
            && a.data.get(32..64) == Some(r.buyer.as_slice())
            && token_amount(&a.data) != Some(0)
        {
            return Err("Jupiter route could consume existing intermediate-token inventory".into());
        }
    }
    Ok(())
}

fn compose(
    c: &Context,
    contract: &InstructionContract,
    quote: &FlashLiquidationQuote,
    repay: u64,
    route: ValidatedRoute,
    payment: Instruction,
    priority_lamports: u64,
) -> Result<Vec<Instruction>> {
    let mut prefix = resources()?;
    // Total priority fee ceiling at the existing 1.4M CU cap.
    let price = priority_lamports
        .checked_mul(1_000_000)
        .ok_or("priority fee overflow")?
        / 1_400_000;
    prefix.push(Instruction {
        program_id: key("ComputeBudget111111111111111111111111111111")?,
        accounts: vec![],
        data: [vec![3], price.to_le_bytes().to_vec()].concat(),
    });
    prefix.extend(route.setup);
    c.flash(
        contract,
        quote,
        FlashPlanOptions {
            max_repayment: repay,
            prefix,
            route: vec![route.swap, payment],
            internal: false,
            min_buyer_credit: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liquidation::plan::Account;
    #[test]
    fn ordinary_composition_keeps_swap_and_fee_aware_payment_inside_each_flash_pair() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let layout = AccountLayoutManifest::from_json(
            &std::fs::read_to_string(root.join("protocol/keeper-account-layout.v1.json")).unwrap(),
        )
        .unwrap();
        let contract = InstructionContract::from_json(
            &std::fs::read_to_string(root.join("protocol/keeper-instructions.v1.json")).unwrap(),
        )
        .unwrap();
        let mut coverage = BTreeSet::new();
        for entry in std::fs::read_dir(root.join("fixtures/litesvm")).unwrap() {
            let v: serde_json::Value =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            if v["action"] != "flash" {
                continue;
            }
            let s: Snapshot = serde_json::from_value(v["snapshot"].clone()).unwrap();
            let c = Context::resolve(
                &layout,
                &s,
                key(v["position"].as_str().unwrap()).unwrap(),
                key(v["buyer"].as_str().unwrap()).unwrap(),
                v["kind"].as_str().unwrap(),
                v["debtAsset"].as_u64().unwrap() as u8,
            )
            .unwrap();
            let q = FlashLiquidationQuote::try_from_slice(
                &crate::runtime::rpc::decode_base64(v["quoteBase64"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
            coverage.insert((c.kind.clone(), c.debt_asset, q.full));
            let (payment, gross) = inventory_payment(&c, &s, q.payment).unwrap();
            let mint = s.account(c.named["debt_mint"]).unwrap();
            let fee = decode_mint_fees(&mint.owner, &mint.data).unwrap();
            assert!(fee.map_or(gross, |f| f.credit(c.epoch, gross, false, false)) >= q.payment);
            if gross > 0 {
                assert!(
                    fee.map_or(gross - 1, |f| f.credit(c.epoch, gross - 1, false, false))
                        < q.payment
                );
            }
            let swap = Instruction {
                program_id: key(PROGRAM).unwrap(),
                accounts: vec![],
                data: vec![42],
            };
            for setup_count in [0, 1, 4] {
                let route = ValidatedRoute {
                    setup: vec![swap.clone(); setup_count],
                    swap: swap.clone(),
                    tables: vec![],
                    minimum_output: gross,
                    expires_at: Instant::now() + std::time::Duration::from_secs(1),
                };
                let ixs = compose(&c, &contract, &q, 100, route, payment.clone(), 1000).unwrap();
                let begin = 3 + setup_count;
                assert_eq!(ixs[begin].program_id, c.program);
                assert_eq!(ixs[begin + 1].program_id, key(PROGRAM).unwrap());
                assert_eq!(ixs[begin + 2].data, payment.data);
                assert_eq!(ixs[begin + 3].program_id, c.program);
                let data = &ixs[begin].data;
                assert_eq!(
                    u16::from_le_bytes(data[data.len() - 2..].try_into().unwrap()) as usize,
                    begin + 3
                );
                assert_eq!(
                    u64::from_le_bytes(data[65..73].try_into().unwrap()),
                    0,
                    "external settlement must not use internal profit field"
                );
                assert_eq!(
                    ixs.last()
                        .unwrap()
                        .accounts
                        .iter()
                        .filter(|a| a.pubkey
                            == c.market
                                .oriented(c.debt_asset == 0)
                                .collateral_reserve_vault)
                        .count(),
                    0
                );
            }
        }
        assert!(coverage.iter().any(|(_, side, _)| *side == 0));
        assert!(coverage.iter().any(|(_, side, _)| *side == 1));
        assert!(coverage.iter().any(|(_, _, full)| *full));
        assert!(coverage.iter().any(|(_, _, full)| !*full));
    }
    #[test]
    fn route_deadline_and_intermediate_inventory_fail_closed() {
        let mut s = Snapshot {
            slot: 100,
            accounts: Default::default(),
        };
        s.accounts.insert(
            PROGRAM.into(),
            Some(Account {
                owner: address([9; 32]),
                data: vec![],
                executable: true,
                lamports: Some(1),
            }),
        );
        let mut r = Request {
            buyer: [1; 32],
            input_mint: [2; 32],
            output_mint: [3; 32],
            source: [4; 32],
            destination: [5; 32],
            input_program: key(TOKEN_PROGRAM).unwrap(),
            output_program: key(TOKEN_PROGRAM).unwrap(),
            amount: 10,
            protected: Default::default(),
        };
        let mut route = ValidatedRoute {
            setup: vec![],
            swap: Instruction {
                program_id: key(PROGRAM).unwrap(),
                accounts: vec![crate::runtime::transaction::AccountMeta::writable([6; 32])],
                data: vec![],
            },
            tables: vec![],
            minimum_output: 10,
            expires_at: Instant::now(),
        };
        let mut data = vec![0; 165];
        data[32..64].copy_from_slice(&r.buyer);
        data[64..72].copy_from_slice(&1u64.to_le_bytes());
        s.accounts.insert(
            address([6; 32]),
            Some(Account {
                owner: TOKEN_PROGRAM.into(),
                data,
                executable: false,
                lamports: Some(1),
            }),
        );
        assert!(validate_snapshot(&route, &r, &s).is_err());
        s.accounts
            .get_mut(&address([6; 32]))
            .unwrap()
            .as_mut()
            .unwrap()
            .data[64..72]
            .fill(0);
        assert!(validate_snapshot(&route, &r, &s).is_ok());
        route.tables.push([7; 32]);
        assert!(validate_snapshot(&route, &r, &s).is_err());
        route.tables.clear();
        route
            .swap
            .accounts
            .push(crate::runtime::transaction::AccountMeta::readonly(
                key(PROGRAM).unwrap(),
            ));
        r.protected.insert(key(PROGRAM).unwrap());
        assert!(validate_snapshot(&route, &r, &s).is_err());
        assert!(
            check_deadline(Some(&RouteState {
                snapshot: s,
                expires_at: Instant::now()
            }))
            .is_err()
        );
        assert!(check_deadline(None).is_ok());
    }
}
