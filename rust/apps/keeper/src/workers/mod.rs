//! Domain workers. Each deployed profile has one explicit entry point.
pub mod hlp;
pub mod lending;
pub mod leverage;
pub mod lifecycle;
pub mod revenue;
pub mod sentinel;

use crate::{
    liquidation::policy::LiquidationMode,
    runtime::execution::{AttemptReport, ExecutionError},
    runtime::rpc::RpcClient,
    runtime::signer::TransactionSigner,
};
use dusk_adapter::{AccountLayoutManifest, DeterministicAccountResolver, InstructionContract};
use std::collections::BTreeSet;

pub struct WorkerContext<'a> {
    pub client: &'a RpcClient,
    pub contract: &'a InstructionContract,
    pub layout: &'a AccountLayoutManifest,
    pub resolver: &'a DeterministicAccountResolver,
    pub signer: &'a dyn TransactionSigner,
    pub program_id: [u8; 32],
    pub dry_run: bool,
    pub max_sends: usize,
    pub priority_markets: &'a BTreeSet<[u8; 32]>,
}

pub fn run(
    profile: &str,
    mode: LiquidationMode,
    context: &WorkerContext<'_>,
) -> Result<Vec<AttemptReport>, ExecutionError> {
    match profile {
        "lending" => lending::run(context, mode),
        "leverage" => leverage::run(context),
        "hlp-recovery" => hlp::HlpJob {
            client: context.client,
            layout: context.layout,
            contract: context.contract,
            signer: context.signer,
            dry_run: context.dry_run,
            max_sends: context.max_sends,
        }
        .run_pass(),
        "auction-arbitrage" => revenue::run(context),
        "lifecycle" => lifecycle::run(context),
        // Read-only observation happens in the common polling loop.
        "sentinel" | "conformance" => Ok(Vec::new()),
        _ => Err(ExecutionError::Assembly(format!(
            "unsupported KEEPER_PROFILE: {profile}"
        ))),
    }
}
