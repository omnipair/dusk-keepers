//! Leveraged-position liquidation followed by delegated conditional orders.
pub mod orders;
use super::WorkerContext;
use crate::{
    liquidation::{LiquidationJob, PositionKind, policy::LiquidationMode},
    runtime::execution::{AttemptReport, ExecutionError},
};
use keeper_core::OutcomeStatus;

pub fn run(context: &WorkerContext<'_>) -> Result<Vec<AttemptReport>, ExecutionError> {
    let mut reports = LiquidationJob {
        client: context.client,
        layout: context.layout,
        contract: context.contract,
        signer: context.signer,
        dry_run: context.dry_run,
        kind: PositionKind::Leverage,
        liquidation_mode: LiquidationMode::Both,
        max_sends: context.max_sends,
        priority_markets: context.priority_markets,
    }
    .run_pass()?;
    let remaining = context.max_sends.saturating_sub(
        reports
            .iter()
            .filter(|report| report.status == OutcomeStatus::Executed)
            .count(),
    );
    if remaining > 0 {
        reports.extend(
            orders::OrderJob {
                client: context.client,
                layout: context.layout,
                contract: context.contract,
                signer: context.signer,
                dry_run: context.dry_run,
                max_sends: remaining,
            }
            .run_pass()?,
        );
    }
    Ok(reports)
}
