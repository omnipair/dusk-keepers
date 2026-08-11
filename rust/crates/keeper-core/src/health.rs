//! Secret-free health and provenance response model.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeeperMode {
    Shadow,
    Live,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLanguage {
    Rust,
    Typescript,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolLockStatus {
    Draft,
    Captured,
    Frozen,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyState {
    Ok,
    Degraded,
    Down,
    Disabled,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParityStatus {
    Matching,
    Mismatched,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthProvenance {
    pub schema_version: u8,
    pub status: HealthStatus,
    pub mode: KeeperMode,
    pub service: String,
    pub profile: String,
    pub runtime: RuntimeProvenance,
    pub protocol: ProtocolProvenance,
    pub dependencies: DependencyHealth,
    pub slots: SlotHealth,
    pub work: WorkHealth,
    pub circuit_breaker: CircuitBreakerHealth,
    pub parity: ParityHealth,
    pub timestamps: HealthTimestamps,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProvenance {
    pub language: RuntimeLanguage,
    pub version: String,
    pub build_sha: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolProvenance {
    pub revision: String,
    pub lock_status: ProtocolLockStatus,
    pub lock_sha256: String,
    pub source_worktree_fingerprint_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyHealth {
    pub rpc: DependencyState,
    pub websocket: DependencyState,
    pub indexer: DependencyState,
    pub database: DependencyState,
    pub lease_store: DependencyState,
    pub signer: DependencyState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotHealth {
    pub rpc: Option<u64>,
    pub indexer: Option<u64>,
    pub last_reconciled: Option<u64>,
    pub lag: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkHealth {
    pub pending: u64,
    pub in_flight: u64,
    pub awaiting_reconciliation: u64,
    pub oldest_attempt_age_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerHealth {
    pub state: CircuitBreakerState,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParityHealth {
    pub status: ParityStatus,
    pub last_compared_at: Option<String>,
    pub mismatch_count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthTimestamps {
    pub started_at: String,
    pub last_scan_at: Option<String>,
    pub last_success_at: Option<String>,
    pub generated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shared_health_provenance_fixture() {
        let health: HealthProvenance = serde_json::from_str(include_str!(
            "../../../../fixtures/conformance/v1/health-provenance.json"
        ))
        .expect("health fixture must parse");
        assert_eq!(health.status, HealthStatus::Degraded);
        assert_eq!(health.protocol.lock_status, ProtocolLockStatus::Captured);
        assert_eq!(health.dependencies.signer, DependencyState::Disabled);
    }
}
