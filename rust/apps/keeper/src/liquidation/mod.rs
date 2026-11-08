//! Shared liquidation mechanics; service policy lives in workers/.
pub mod cli;
mod engine;
pub mod plan;
pub mod policy;
mod position_log;
mod search;
pub mod simulation;
mod validation;

pub use engine::{LiquidationJob, PositionKind};
pub(crate) use validation::{compute_limit_rejection, post_bytes, required_u64, state_hash};
