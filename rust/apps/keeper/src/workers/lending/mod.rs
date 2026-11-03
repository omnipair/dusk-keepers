//! Borrowing positions: one health scan, ordinary-first route selection.
use super::WorkerContext;
use crate::{
    liquidation::{LiquidationJob, PositionKind, policy::LiquidationMode},
    runtime::execution::{AttemptReport, ExecutionError},
};

pub fn run(
    context: &WorkerContext<'_>,
    mode: LiquidationMode,
) -> Result<Vec<AttemptReport>, ExecutionError> {
    LiquidationJob {
        client: context.client,
        layout: context.layout,
        contract: context.contract,
        signer: context.signer,
        dry_run: context.dry_run,
        kind: PositionKind::Borrowing,
        liquidation_mode: mode,
        max_sends: context.max_sends,
        priority_markets: context.priority_markets,
    }
    .run_pass()
}
