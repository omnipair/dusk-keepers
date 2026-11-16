//! Actual finalized accounting from the receipt already fetched by the journal.
//! Formatting is best-effort, never authorizes a trade, and makes no RPC calls.
mod events;
mod wire;

use super::logging::{self, CYAN, DIM, GREEN, RED, YELLOW, clean, paint, signed, units};
use events::{Event, Protocol};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    io::Read,
};

const TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const SYSTEM: &str = "11111111111111111111111111111111";

pub struct Context<'a> {
    pub signature: &'a str,
    pub action: &'a str,
    pub position: &'a str,
    pub market: &'a str,
}

pub fn finalized(context: &Context<'_>, receipt: &Value, bytes: &[u8]) {
    match render(context, receipt, bytes) {
        Ok(text) => logging::emit(text),
        Err(error) => logging::emit(paint(
            format!(
                "receipt_details_unavailable signature={} detail={} (journal outcome preserved)",
                clean(context.signature),
                clean(&error)
            ),
            YELLOW,
        )),
    }
}

/// Replay a saved finalized receipt without a signer, RPC, or database access.
pub fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw = String::new();
    std::io::stdin()
        .take(8 * 1024 * 1024)
        .read_to_string(&mut raw)?;
    let input: Value = serde_json::from_str(&raw)?;
    let receipt = &input["receipt"];
    if receipt["transaction"][1] != "base64" {
        return Err("receipt transaction must be base64".into());
    }
    let bytes = super::rpc::decode_base64(
        receipt["transaction"][0]
            .as_str()
            .ok_or("receipt transaction missing")?,
    )
    .ok_or("invalid receipt encoding")?;
    let context = Context {
        signature: input["signature"].as_str().ok_or("signature missing")?,
        action: input["action"].as_str().unwrap_or("transaction"),
        position: input["position"].as_str().unwrap_or("n/a"),
        market: input["market"].as_str().unwrap_or("n/a"),
    };
    logging::emit(render(&context, receipt, &bytes)?);
    Ok(())
}

#[derive(Clone)]
struct TokenInfo {
    mint: String,
    decimals: u8,
    owner: Option<String>,
}
struct Balance {
    account: String,
    token: TokenInfo,
    before: u64,
    after: u64,
}
struct Transfer {
    kind: &'static str,
    source: Option<String>,
    destination: Option<String>,
    mint: Option<String>,
    amount: u64,
    fee: Option<u64>,
    path: String,
}
#[derive(Default)]
struct Ledger {
    tokens: BTreeMap<String, TokenInfo>,
    decimals: BTreeMap<String, u8>,
    roles: BTreeMap<String, BTreeSet<String>>,
    markets: BTreeMap<(String, u8), String>,
    payer: String,
}
impl Ledger {
    fn label(&self, address: &str) -> String {
        let role = if address == self.payer {
            "keeper".to_owned()
        } else {
            self.roles
                .get(address)
                .map(|r| r.iter().cloned().collect::<Vec<_>>().join("/"))
                .unwrap_or_else(|| "account".into())
        };
        format!("{role} ({address})")
    }
    fn token(&self, mint: &str, amount: u128) -> String {
        match self.decimals.get(mint) {
            Some(decimals) => format!("{} mint={mint}", units(amount, *decimals)),
            None => format!("{amount} raw atoms mint={mint} (decimals unavailable)"),
        }
    }
    fn asset(&self, market: &[u8; 32], side: u8, amount: u64) -> String {
        let market = bs58::encode(market).into_string();
        let label = match side {
            0 => "BASE",
            1 => "QUOTE",
            _ => "UNKNOWN",
        };
        self.markets.get(&(market, side)).map_or_else(
            || format!("{amount} raw atoms {label} (mint/decimals unavailable)"),
            |mint| format!("{} {label}", self.token(mint, u128::from(amount))),
        )
    }
    fn add_roles(&mut self, protocol: &Protocol, instructions: &[wire::Instruction]) {
        for ix in instructions {
            let Some((name, names)) = protocol.accounts(ix) else {
                continue;
            };
            let named: BTreeMap<_, _> = names
                .iter()
                .zip(&ix.accounts)
                .map(|(n, a)| (n.as_str(), a))
                .collect();
            for (name, address) in &named {
                if **address != protocol.program {
                    self.roles
                        .entry((*address).clone())
                        .or_default()
                        .insert((*name).to_owned());
                }
            }
            let side = match name {
                "emergency_liquidation" => ix.data.get(8),
                "begin_flash_liquidation" => ix.data.get(40),
                _ => None,
            };
            if let (Some(side @ 0..=1), Some(market), Some(debt), Some(collateral)) = (
                side,
                named.get("market"),
                named.get("debt_mint"),
                named.get("collateral_mint"),
            ) {
                self.markets
                    .insert(((*market).to_string(), *side), (*debt).to_string());
                self.markets
                    .insert(((*market).to_string(), 1 - side), (*collateral).to_string());
            }
        }
    }
}

fn balances(meta: &Value, keys: &[String]) -> Result<Vec<Balance>, String> {
    let mut entries: BTreeMap<usize, (TokenInfo, u64, u64)> = BTreeMap::new();
    for (field, before) in [("preTokenBalances", true), ("postTokenBalances", false)] {
        for value in meta[field]
            .as_array()
            .ok_or("token balance metadata unavailable")?
        {
            let index = value["accountIndex"]
                .as_u64()
                .ok_or("token account index missing")? as usize;
            let mint = value["mint"]
                .as_str()
                .ok_or("token mint missing")?
                .to_owned();
            let decimals = u8::try_from(
                value["uiTokenAmount"]["decimals"]
                    .as_u64()
                    .ok_or("token decimals missing")?,
            )
            .map_err(|_| "invalid token decimals")?;
            let amount = value["uiTokenAmount"]["amount"]
                .as_str()
                .ok_or("token atoms missing")?
                .parse::<u64>()
                .map_err(|_| "invalid token atoms")?;
            let token = TokenInfo {
                mint,
                decimals,
                owner: value["owner"].as_str().map(str::to_owned),
            };
            let entry = entries
                .entry(index)
                .or_insert_with(|| (token.clone(), 0, 0));
            if entry.0.mint != token.mint || entry.0.decimals != token.decimals {
                return Err("token identity changed in receipt".into());
            }
            if let (Some(previous), Some(current)) = (&entry.0.owner, &token.owner)
                && previous != current
            {
                return Err("token owner changed in receipt; wallet totals unavailable".into());
            }
            if entry.0.owner.is_none() {
                entry.0.owner = token.owner;
            }
            if before {
                entry.1 = amount;
            } else {
                entry.2 = amount;
            }
        }
    }
    entries
        .into_iter()
        .map(|(index, (token, before, after))| {
            Ok(Balance {
                account: keys
                    .get(index)
                    .ok_or("token account index out of range")?
                    .clone(),
                token,
                before,
                after,
            })
        })
        .collect()
}

fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn transfer(ix: &wire::Instruction, ledger: &mut Ledger) -> Option<Transfer> {
    let mut out = Transfer {
        kind: "TRANSFER",
        source: None,
        destination: None,
        mint: None,
        amount: 0,
        fee: None,
        path: ix.path.clone(),
    };
    if ix.program == SYSTEM {
        let op = u32::from_le_bytes(ix.data.get(..4)?.try_into().ok()?);
        if !matches!(op, 0 | 2) {
            return None;
        }
        out.kind = if op == 0 {
            "ACCOUNT FUNDING"
        } else {
            "SOL TRANSFER"
        };
        out.source = Some(ix.accounts.first()?.clone());
        out.destination = Some(ix.accounts.get(1)?.clone());
        out.mint = Some("SOL".into());
        out.amount = u64_at(&ix.data, 4)?;
        return Some(out);
    }
    if ix.program != TOKEN && ix.program != TOKEN_2022 {
        return None;
    }
    let op = *ix.data.first()?;
    let checked = matches!(op, 12 | 14 | 15) || (op == 26 && ix.data.get(1) == Some(&1));
    let (source, destination, mint) = match op {
        3 => (Some(0), Some(1), None),
        12 | 26 if checked => (Some(0), Some(2), Some(1)),
        7 | 14 => {
            out.kind = "MINT";
            (None, Some(1), Some(0))
        }
        8 | 15 => {
            out.kind = "BURN";
            (Some(0), None, Some(1))
        }
        _ => return None,
    };
    let offset = if op == 26 { 2 } else { 1 };
    out.amount = u64_at(&ix.data, offset)?;
    out.source = source.and_then(|i| ix.accounts.get(i).cloned());
    out.destination = destination.and_then(|i| ix.accounts.get(i).cloned());
    out.mint = mint.and_then(|i| ix.accounts.get(i).cloned());
    if source.is_some() && out.source.is_none()
        || destination.is_some() && out.destination.is_none()
    {
        return None;
    }
    if let (true, Some(mint), Some(decimals)) =
        (checked, out.mint.as_ref(), ix.data.get(offset + 8))
    {
        ledger.decimals.entry(mint.clone()).or_insert(*decimals);
        for account in [&out.source, &out.destination].into_iter().flatten() {
            ledger
                .tokens
                .entry(account.clone())
                .or_insert_with(|| TokenInfo {
                    mint: mint.clone(),
                    decimals: *decimals,
                    owner: None,
                });
        }
    }
    if op == 26 {
        out.fee = u64_at(&ix.data, 11);
    }
    Some(out)
}

fn render(context: &Context<'_>, receipt: &Value, bytes: &[u8]) -> Result<String, String> {
    let meta = &receipt["meta"];
    let failed = !meta
        .get("err")
        .ok_or("receipt status unavailable")?
        .is_null();
    let tx = wire::decode(bytes, meta)?;
    let mut ledger = Ledger {
        payer: tx.keys[0].clone(),
        ..Ledger::default()
    };
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        paint(
            format!(
                "=== {} | {} ===",
                if failed {
                    "FINALIZED FAILED"
                } else {
                    "FINALIZED SUCCESS"
                },
                clean(context.action)
            ),
            if failed { RED } else { GREEN }
        )
    )
    .unwrap();
    writeln!(
        out,
        "  Signature: {}\n  Slot: {}\n  Keeper wallet: {}\n  Position: {}\n  Market: {}",
        clean(context.signature),
        receipt["slot"],
        ledger.payer,
        clean(context.position),
        clean(context.market)
    )
    .unwrap();
    if failed {
        writeln!(out, "  Error: {}", clean(&meta["err"].to_string())).unwrap();
    }
    let fee = meta["fee"].as_u64();
    writeln!(
        out,
        "  {}",
        paint(
            format!(
                "SOL fee paid: {}",
                fee.map_or_else(
                    || "unavailable".into(),
                    |n| format!(
                        "{} SOL ({n} lamports; includes priority fee)",
                        units(u128::from(n), 9)
                    )
                )
            ),
            YELLOW
        )
    )
    .unwrap();
    if let (Some(before), Some(after)) = (
        meta["preBalances"][0].as_u64(),
        meta["postBalances"][0].as_u64(),
    ) {
        let delta = i128::from(after) - i128::from(before);
        writeln!(
            out,
            "  Wallet SOL: {} -> {} | net {} SOL",
            units(u128::from(before), 9),
            units(u128::from(after), 9),
            signed(delta, 9)
        )
        .unwrap();
        if let Some(fee) = fee {
            writeln!(
                out,
                "  Other wallet SOL movement: {} SOL (rent/refunds/transfers, excludes fee)",
                signed(delta + i128::from(fee), 9)
            )
            .unwrap();
        }
    }
    if let Some(cu) = meta["computeUnitsConsumed"].as_u64() {
        writeln!(out, "  Compute used: {cu} CU").unwrap();
    }
    let token_balances = balances(meta, &tx.keys);
    if let Ok(balances) = &token_balances {
        for b in balances {
            ledger.tokens.insert(b.account.clone(), b.token.clone());
            ledger
                .decimals
                .insert(b.token.mint.clone(), b.token.decimals);
        }
    }
    let protocol = Protocol::load();
    if let Ok(protocol) = protocol {
        ledger.add_roles(protocol, &tx.instructions);
    }
    let transfers: Vec<_> = tx
        .instructions
        .iter()
        .filter_map(|ix| transfer(ix, &mut ledger))
        .collect();
    if let Ok(balances) = &token_balances {
        let mut wallet: BTreeMap<String, (u128, u128, u8)> = BTreeMap::new();
        for b in balances
            .iter()
            .filter(|b| b.token.owner.as_deref() == Some(&ledger.payer))
        {
            let totals = wallet
                .entry(b.token.mint.clone())
                .or_insert((0, 0, b.token.decimals));
            totals.0 += u128::from(b.before);
            totals.1 += u128::from(b.after);
        }
        writeln!(
            out,
            "{}",
            paint(
                "  KEEPER TOKEN BALANCES (actual net, separate from protocol repayment)",
                CYAN
            )
        )
        .unwrap();
        if wallet.is_empty() {
            writeln!(out, "    No keeper-owned token balances recorded.").unwrap();
        }
        for (mint, (before, after, decimals)) in wallet {
            let delta = after as i128 - before as i128;
            writeln!(
                out,
                "    {} | {} -> {} | mint={mint}",
                paint(
                    format!(
                        "{} {}",
                        if delta < 0 {
                            "NET PAID"
                        } else if delta > 0 {
                            "NET RECEIVED"
                        } else {
                            "UNCHANGED"
                        },
                        signed(delta, decimals)
                    ),
                    if delta < 0 { YELLOW } else { GREEN }
                ),
                units(before, decimals),
                units(after, decimals)
            )
            .unwrap();
        }
    } else {
        writeln!(
            out,
            "  Token balance details unavailable; no zero balances assumed."
        )
        .unwrap();
    }
    if !failed {
        match protocol {
            Ok(protocol) => {
                for ix in &tx.instructions {
                    if let Some(event) = protocol.event(ix) {
                        match event {
                            Ok(event) => render_event(&mut out, &ledger, event),
                            Err(error) => {
                                writeln!(out, "  Event details unavailable: {}", clean(&error))
                                    .unwrap();
                            }
                        }
                    }
                    if ix.program == crate::routing::jupiter::PROGRAM && !ix.path.contains('.') {
                        writeln!(out, "{}", paint(format!("  SWAP ROUTE: Jupiter | instruction {} | actual hops in transfers below", ix.path), CYAN)).unwrap();
                    }
                }
            }
            Err(_) => {
                writeln!(
                    out,
                    "  Dusk event details unavailable (pinned IDL could not be loaded)."
                )
                .unwrap();
            }
        }
        writeln!(out, "{}", paint("  TRANSFERS / MINTS / BURNS (instruction amounts; token transfer fees may reduce recipient credit)", CYAN)).unwrap();
        if meta["innerInstructions"].is_null() {
            writeln!(
                out,
                "    Inner instructions unavailable; only top-level transfers can be shown."
            )
            .unwrap();
        }
        for t in &transfers {
            let mint = t.mint.as_deref().or_else(|| {
                [&t.source, &t.destination]
                    .into_iter()
                    .flatten()
                    .find_map(|a| ledger.tokens.get(a).map(|t| t.mint.as_str()))
            });
            let amount = match mint {
                Some("SOL") => format!("{} SOL", units(u128::from(t.amount), 9)),
                Some(mint) => ledger.token(mint, u128::from(t.amount)),
                None => format!("{} raw atoms (mint unavailable)", t.amount),
            };
            writeln!(
                out,
                "    [{}] {} {}\n      {} -> {}",
                t.path,
                t.kind,
                amount,
                t.source
                    .as_deref()
                    .map_or_else(|| "token supply".into(), |a| ledger.label(a)),
                t.destination
                    .as_deref()
                    .map_or_else(|| "token supply".into(), |a| ledger.label(a))
            )
            .unwrap();
            if let (Some(fee), Some(mint)) = (t.fee, mint) {
                writeln!(
                    out,
                    "      Explicit transfer fee: {}",
                    ledger.token(mint, u128::from(fee))
                )
                .unwrap();
            }
        }
    } else {
        writeln!(out, "  Swap events and attempted transfers were rolled back; they are not reported as payments or rewards.").unwrap();
    }
    if let Ok(balances) = &token_balances {
        writeln!(
            out,
            "{}",
            paint("  TOKEN ACCOUNT CHANGES (before -> after)", CYAN)
        )
        .unwrap();
        for b in balances.iter().filter(|b| b.before != b.after) {
            writeln!(
                out,
                "    {} | {} -> {} | net {} | mint={} owner={}",
                ledger.label(&b.account),
                units(u128::from(b.before), b.token.decimals),
                units(u128::from(b.after), b.token.decimals),
                signed(i128::from(b.after) - i128::from(b.before), b.token.decimals),
                b.token.mint,
                b.token.owner.as_deref().unwrap_or("unavailable")
            )
            .unwrap();
        }
    }
    writeln!(
        out,
        "{}",
        paint(
            "  SOL ACCOUNT CHANGES (includes rent and account closure refunds)",
            CYAN
        )
    )
    .unwrap();
    for (i, address) in tx.keys.iter().enumerate() {
        if let (Some(before), Some(after)) = (
            meta["preBalances"][i].as_u64(),
            meta["postBalances"][i].as_u64(),
        ) && before != after
        {
            writeln!(
                out,
                "    {} | {} -> {} | net {} SOL",
                ledger.label(address),
                units(u128::from(before), 9),
                units(u128::from(after), 9),
                signed(i128::from(after) - i128::from(before), 9)
            )
            .unwrap();
        }
    }
    writeln!(out, "{}", paint("=== END RECEIPT ===", DIM)).unwrap();
    Ok(out.trim_end().to_owned())
}

fn render_event(out: &mut String, ledger: &Ledger, event: Event) {
    match event {
        Event::Swap(q) => {
            writeln!(out, "{}", paint("  SWAP EXECUTED: Dusk", CYAN)).unwrap();
            writeln!(
                out,
                "    IN  {}\n    OUT {}\n    Curve output before trading fees: {}",
                ledger.asset(&q.market, q.asset_in_side, q.amount_in),
                ledger.asset(&q.market, 1u8.wrapping_sub(q.asset_in_side), q.amount_out),
                ledger.asset(
                    &q.market,
                    1u8.wrapping_sub(q.asset_in_side),
                    q.gross_amount_out
                )
            )
            .unwrap();
            writeln!(
                out,
                "    Input applied to curve: {}",
                ledger.asset(&q.market, q.asset_in_side, q.amount_in_after_fee)
            )
            .unwrap();
            for (name, amount) in [
                ("Base trading fee", q.base_fee),
                ("Divergence fee", q.divergence_fee),
                ("Volatility fee", q.volatility_fee),
                ("Retained fee allocation", q.retained_fee),
                ("Compounded LP fee allocation", q.compounded_fee),
            ] {
                writeln!(
                    out,
                    "    {name}: {}",
                    ledger.asset(&q.market, q.fee_asset_side, amount)
                )
                .unwrap();
            }
        }
        Event::Emergency(e) => {
            let q = &e.quote;
            writeln!(
                out,
                "{}",
                paint(
                    format!(
                        "  EMERGENCY LIQUIDATION | position={}",
                        bs58::encode(e.position).into_string()
                    ),
                    CYAN
                )
            )
            .unwrap();
            writeln!(
                out,
                "    {} | recipient={}",
                paint(
                    format!(
                        "REWARD RECEIVED: {}",
                        ledger.asset(&e.market, e.debt_asset, q.reward_credit)
                    ),
                    GREEN
                ),
                bs58::encode(e.caller).into_string()
            )
            .unwrap();
            writeln!(
                out,
                "    Collateral spent by position: {}\n    Collateral credited to swap: {}",
                ledger.asset(
                    &e.market,
                    1u8.wrapping_sub(e.debt_asset),
                    q.collateral_debit
                ),
                ledger.asset(
                    &e.market,
                    1u8.wrapping_sub(e.debt_asset),
                    q.collateral_credit
                )
            )
            .unwrap();
            for (name, value) in [
                ("Swap output", q.swap_output),
                ("Debt repaid from swap proceeds", q.repayment),
                ("Liquidation fee total", q.fee.total),
                ("Protocol fee allocation", q.fee.protocol),
                ("Junior fee allocation", q.fee.junior),
                ("Owner received", q.owner_credit),
                ("Principal written off", q.loss.principal_written_off),
                ("Interest canceled", q.loss.interest_canceled),
                ("Remaining debt", e.remaining_debt),
            ] {
                writeln!(
                    out,
                    "    {name}: {}",
                    ledger.asset(&e.market, e.debt_asset, value)
                )
                .unwrap();
            }
            writeln!(
                out,
                "    Remaining collateral: {}\n    Position owner: {}",
                ledger.asset(
                    &e.market,
                    1u8.wrapping_sub(e.debt_asset),
                    e.remaining_collateral
                ),
                bs58::encode(e.owner).into_string()
            )
            .unwrap();
        }
        Event::Flash(e) => {
            writeln!(
                out,
                "{}",
                paint(
                    format!(
                        "  FLASH LIQUIDATION | position={} buyer={}",
                        bs58::encode(e.position).into_string(),
                        bs58::encode(e.buyer).into_string()
                    ),
                    CYAN
                )
            )
            .unwrap();
            writeln!(
                out,
                "    Collateral spent by position: {}",
                ledger.asset(
                    &e.market,
                    1u8.wrapping_sub(e.debt_asset),
                    e.collateral_debit
                )
            )
            .unwrap();
            for (name, value) in [
                ("Debt repaid", e.debt_repaid),
                ("Owner surplus", e.owner_surplus),
                ("Liquidation fee total", e.fee.total),
                ("Protocol fee allocation", e.fee.protocol),
                ("Junior fee allocation", e.fee.junior),
                ("Principal written off", e.loss.principal_written_off),
                ("Interest canceled", e.loss.interest_canceled),
                ("Remaining debt", e.remaining_debt),
            ] {
                writeln!(
                    out,
                    "    {name}: {}",
                    ledger.asset(&e.market, e.debt_asset, value)
                )
                .unwrap();
            }
            writeln!(
                out,
                "    Remaining collateral: {}",
                ledger.asset(
                    &e.market,
                    1u8.wrapping_sub(e.debt_asset),
                    e.remaining_collateral
                )
            )
            .unwrap();
        }
    }
}
