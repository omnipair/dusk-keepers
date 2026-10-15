//! Shared execution outcomes and error classification.
use crate::discovery::DiscoveryError;
use keeper_core::{OutcomeStatus, ReasonCode, lifecycle::ExpectedRace};
use std::{error::Error, fmt};

#[derive(Debug)]
pub enum ExecutionError {
    Discovery(DiscoveryError),
    Encoding(String),
    Assembly(String),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery(error) => write!(formatter, "{error}"),
            Self::Encoding(detail) => write!(formatter, "instruction encoding failed: {detail}"),
            Self::Assembly(detail) => write!(formatter, "transaction assembly failed: {detail}"),
        }
    }
}

impl Error for ExecutionError {}

impl From<DiscoveryError> for ExecutionError {
    fn from(error: DiscoveryError) -> Self {
        Self::Discovery(error)
    }
}

/// What one attempt on one candidate came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptReport {
    pub position: String,
    pub debt_mint: String,
    pub status: OutcomeStatus,
    pub reason: ReasonCode,
    pub race: Option<ExpectedRace>,
    pub signature: Option<String>,
    pub detail: Option<String>,
}

/// Map a program error seen during simulation onto the lifecycle vocabulary.
///
/// The distinction that matters is between "this position is fine" and "this
/// keeper is broken". The first is the overwhelmingly common answer and must
/// stay quiet; the second has to be loud. Anchor error codes are matched as
/// text because that is what simulation returns.
pub fn classify_simulation_failure(detail: &str) -> (ReasonCode, Option<ExpectedRace>) {
    if detail.contains("PositionNotLiquidatable") {
        return (
            ReasonCode::BoundsNotMet,
            Some(ExpectedRace::ObligationNoLongerLiquidatable),
        );
    }
    if detail.contains("InvalidBorrowPosition") || detail.contains("AccountNotInitialized") {
        return (ReasonCode::StateChanged, Some(ExpectedRace::AccountChanged));
    }
    if detail.contains("InvalidSettlementPrice") {
        return (
            ReasonCode::BoundsNotMet,
            Some(ExpectedRace::ObligationNoLongerLiquidatable),
        );
    }
    if detail.contains("LiquidationRepayTooLarge") {
        return (ReasonCode::BoundsNotMet, None);
    }
    if detail.contains("InvalidAssetMint") || detail.contains("InvalidMint") {
        return (ReasonCode::BoundsNotMet, None);
    }
    if detail.contains("BrokenInvariant") {
        return (ReasonCode::UnsupportedProtocol, None);
    }
    if detail.contains("BlockhashNotFound") {
        return (ReasonCode::BlockhashExpired, None);
    }
    if detail.contains("insufficient") || detail.contains("InsufficientFunds") {
        return (ReasonCode::InsufficientSignerBalance, None);
    }
    (ReasonCode::SimulationRejected, None)
}
