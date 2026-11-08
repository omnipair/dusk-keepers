//! Optional position diagnostics. Never used to authorize or size execution.
use super::plan::{Context, Snapshot, address};
use dusk_adapter::liquidation::LiquidationHealth;

pub fn enabled() -> bool {
    std::env::var("KEEPER_LOG_POSITIONS").as_deref() == Ok("1")
}

// String arithmetic preserves token atoms, including values above JS/f64 precision.
fn units(value: u128, decimals: u8) -> String {
    if decimals == 0 {
        return value.to_string();
    }
    let mut digits = format!("{value:0>width$}", width = usize::from(decimals) + 1);
    digits.insert(digits.len() - usize::from(decimals), '.');
    digits
}

fn ratio(numerator: u128, denominator: u128, decimals: u8) -> String {
    if denominator == 0 {
        return "n/a".into();
    }
    units(
        numerator * 10u128.pow(u32::from(decimals)) / denominator,
        decimals,
    )
}

fn health_text(health: &LiquidationHealth) -> String {
    let maximum_ltv = 10_000u128.saturating_sub(u128::from(health.maintenance_bps));
    let state = if health.debt == 0 {
        "DEBT_FREE"
    } else if health.eligible() {
        "LIQUIDATABLE"
    } else {
        "HEALTHY"
    };
    let factor = ratio(
        u128::from(health.collateral_value) * maximum_ltv,
        u128::from(health.debt) * 10_000,
        6,
    );
    let ltv = ratio(
        u128::from(health.debt) * 100,
        u128::from(health.collateral_value),
        4,
    );
    format!(
        "state={state} health_factor={factor} liquidation_at=1.000000 ltv_pct={ltv} liquidation_ltv_pct={} maintenance_pct={} eligible={}",
        units(maximum_ltv, 2),
        units(u128::from(health.maintenance_bps), 2),
        health.eligible()
    )
}

fn amount(snapshot: &Snapshot, mint: [u8; 32], value: u64) -> String {
    match snapshot
        .account(mint)
        .ok()
        .and_then(|a| a.data.get(44))
        .copied()
    {
        Some(decimals) => units(u128::from(value), decimals),
        None => format!("{value} raw-units (decimals unavailable)"),
    }
}

pub fn health(
    context: &Context,
    snapshot: &Snapshot,
    health: Option<&LiquidationHealth>,
    preview_slot: Option<u64>,
    monitor_reason: Option<&str>,
) {
    if !enabled() {
        return;
    }
    let debt_mint = context.named["debt_mint"];
    let collateral_mint = context.named["collateral_mint"];
    let (debt_side, collateral_side) = if context.debt_asset == 0 {
        ("BASE", "QUOTE")
    } else {
        ("QUOTE", "BASE")
    };
    let debt = health.map_or(0, |h| h.debt);
    let value = health.map_or_else(
        || "n/a".into(),
        |h| amount(snapshot, debt_mint, h.collateral_value),
    );
    let metrics = health.map_or_else(
        || "state=DEBT_FREE health_factor=n/a eligible=false".into(),
        health_text,
    );
    let scope = monitor_reason.unwrap_or("supported market");
    println!(
        "position_health kind={} position={}\n  owner={} market={} scope={scope:?}\n  debt={} {debt_side} debt_raw={debt} collateral={} {collateral_side} collateral_raw={} collateral_value={value} {debt_side} debt_shares={}\n  {metrics}\n  debt_mint={} collateral_mint={} snapshot_slot={} preview_slot={}",
        context.kind,
        address(context.position),
        address(context.owner),
        address(context.market.address),
        amount(snapshot, debt_mint, debt),
        amount(snapshot, collateral_mint, context.collateral),
        context.collateral,
        context.shares,
        address(debt_mint),
        address(collateral_mint),
        snapshot.slot,
        preview_slot.map_or_else(|| "n/a".into(), |slot| slot.to_string()),
    );
}

pub fn unavailable(
    position: [u8; 32],
    market: [u8; 32],
    owner: Option<[u8; 32]>,
    kind: &str,
    reason: &str,
) {
    if enabled() {
        println!(
            "position_health kind={kind} position={} owner={} market={} state=UNAVAILABLE reason={reason:?}",
            address(position),
            owner.map_or_else(|| "unknown".into(), address),
            address(market),
        );
    }
}

pub fn decision(position: [u8; 32], side: u8, detail: &str, signature: Option<&str>) {
    if enabled() {
        println!(
            "position_decision position={} debt_side={} detail={detail:?} signature={}",
            address(position),
            if side == 0 { "BASE" } else { "QUOTE" },
            signature.unwrap_or("none")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_large_and_small_token_amounts_with_different_decimals() {
        assert_eq!(units(u128::from(u64::MAX), 6), "18446744073709.551615");
        assert_eq!(units(1, 9), "0.000000001");
        assert_eq!(units(42, 0), "42");
        assert_eq!(units(1, 20), "0.00000000000000000001");
    }

    #[test]
    fn reports_exact_liquidation_boundary_and_healthy_debt() {
        let mut h = LiquidationHealth {
            collateral_value: 100_000_000,
            debt: 90_000_000,
            maintenance_bps: 1000,
        };
        let text = health_text(&h);
        assert!(text.contains("state=LIQUIDATABLE health_factor=1.000000"));
        assert!(text.contains(
            "ltv_pct=90.0000 liquidation_ltv_pct=90.00 maintenance_pct=10.00 eligible=true"
        ));
        h.debt = 80_000_000;
        assert!(health_text(&h).contains("state=HEALTHY health_factor=1.125000"));
    }

    #[test]
    fn never_invents_health_ratios_for_zero_debt_or_zero_value() {
        let mut h = LiquidationHealth {
            collateral_value: 100,
            debt: 0,
            maintenance_bps: 1000,
        };
        assert!(health_text(&h).contains("state=DEBT_FREE health_factor=n/a"));
        h.debt = 10;
        h.collateral_value = 0;
        assert!(health_text(&h).contains("state=LIQUIDATABLE health_factor=0.000000"));
        assert!(health_text(&h).contains("ltv_pct=n/a"));
    }
}
