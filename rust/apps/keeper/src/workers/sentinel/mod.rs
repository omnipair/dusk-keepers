//! Non-signing market/position discovery and transfer-fee monitoring.
pub mod fee_notice;
use crate::runtime::rpc::{DiscoveryError, RpcClient, account_discriminator};
/// What a discovery pass observed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub markets: usize,
    pub borrow_positions: usize,
    pub leverage_positions: usize,
    pub slot: u64,
}

/// One discovery pass over the pinned programs.
pub fn observe(client: &RpcClient, dusk_program_id: &str) -> Result<Observation, DiscoveryError> {
    let slot = client.slot()?;
    let markets = client.count_accounts(dusk_program_id, account_discriminator("Market"))?;
    let borrow_positions =
        client.count_accounts(dusk_program_id, account_discriminator("BorrowPosition"))?;
    let leverage_positions =
        client.count_accounts(dusk_program_id, account_discriminator("LeveragePosition"))?;
    Ok(Observation {
        borrow_positions,
        leverage_positions,
        markets,
        slot,
    })
}
