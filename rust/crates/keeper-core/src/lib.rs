//! Chain-independent keeper scheduling and execution contracts.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub mod health;
pub mod lifecycle;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    LeverageLiquidation,
    LendingLiquidationTrigger,
    LendingLiquidationBid,
    LendingLiquidationSettle,
    StopLoss,
    TakeProfit,
    AuctionBid,
    Lifecycle,
    Sentinel,
}

impl JobKind {
    pub const fn priority(self) -> u16 {
        match self {
            Self::LeverageLiquidation | Self::LendingLiquidationSettle => 500,
            Self::LendingLiquidationTrigger => 490,
            Self::StopLoss => 400,
            Self::TakeProfit => 300,
            Self::LendingLiquidationBid | Self::AuctionBid => 200,
            Self::Lifecycle => 100,
            Self::Sentinel => 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateIntent {
    pub candidate_id: String,
    pub job_kind: JobKind,
    pub conflict_key: String,
    pub market: String,
    pub target: String,
    pub observed_slot: u64,
    pub protocol_revision: String,
    pub expected_state_hash: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueResult {
    Accepted,
    Superseded,
    ConflictLocked,
    RevisionMismatch,
}

#[derive(Debug)]
pub struct Scheduler {
    protocol_revision: String,
    pending: BTreeMap<String, CandidateIntent>,
    in_flight: BTreeSet<String>,
}

impl Scheduler {
    pub fn new(protocol_revision: impl Into<String>) -> Self {
        Self {
            protocol_revision: protocol_revision.into(),
            pending: BTreeMap::new(),
            in_flight: BTreeSet::new(),
        }
    }

    pub fn enqueue(&mut self, candidate: CandidateIntent) -> EnqueueResult {
        if candidate.protocol_revision != self.protocol_revision {
            return EnqueueResult::RevisionMismatch;
        }
        if self.in_flight.contains(&candidate.conflict_key) {
            return EnqueueResult::ConflictLocked;
        }

        let replace = match self.pending.get(&candidate.conflict_key) {
            None => true,
            Some(current) => candidate_rank(&candidate) > candidate_rank(current),
        };
        if replace {
            self.pending
                .insert(candidate.conflict_key.clone(), candidate);
            EnqueueResult::Accepted
        } else {
            EnqueueResult::Superseded
        }
    }

    pub fn pop_next(&mut self) -> Option<CandidateIntent> {
        let conflict_key = self
            .pending
            .iter()
            .max_by(|(_, left), (_, right)| candidate_rank(left).cmp(&candidate_rank(right)))
            .map(|(key, _)| key.clone())?;
        let candidate = self.pending.remove(&conflict_key)?;
        self.in_flight.insert(conflict_key);
        Some(candidate)
    }

    /// Used only after an external lease has been acquired. In-memory locking is
    /// a second line of defense, never the distributed coordination mechanism.
    pub fn mark_in_flight(&mut self, conflict_key: impl Into<String>) {
        let key = conflict_key.into();
        self.pending.remove(&key);
        self.in_flight.insert(key);
    }

    pub fn finish(&mut self, conflict_key: &str) -> bool {
        self.in_flight.remove(conflict_key)
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

fn candidate_rank(candidate: &CandidateIntent) -> (u16, u64, &str) {
    (
        candidate.job_kind.priority(),
        candidate.observed_slot,
        candidate.candidate_id.as_str(),
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Executed,
    Skipped,
    RetryableFailure,
    TerminalFailure,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    Confirmed,
    ShadowMode,
    StaleObservation,
    StateChanged,
    AlreadyResolved,
    BoundsNotMet,
    PolicyDenied,
    LeaseNotAcquired,
    SimulationRejected,
    SigningDenied,
    RpcUnavailable,
    BlockhashExpired,
    ConfirmationUnknown,
    PostconditionFailed,
    InsufficientSignerBalance,
    ProtocolRevisionMismatch,
    UnsupportedProtocol,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOutcome {
    pub attempt_id: String,
    pub attempt_number: u32,
    pub work_key: String,
    pub candidate_id: String,
    pub job_kind: JobKind,
    pub conflict_key: String,
    pub status: OutcomeStatus,
    pub reason_code: ReasonCode,
    pub race_code: Option<lifecycle::ExpectedRace>,
    pub observed_slot: u64,
    pub finalized_slot: Option<u64>,
    pub protocol_revision: String,
    pub signature: Option<String>,
    pub signing_generation: u32,
}

/// External ports are kept explicit so retries, leases, and chain access are
/// replaceable and independently testable.
pub mod ports {
    use super::{CandidateIntent, ExecutionOutcome, lifecycle::LifecycleEvent};

    pub trait CandidateSource {
        type Error;
        fn discover(&self) -> Result<Vec<CandidateIntent>, Self::Error>;
    }

    pub trait StateRevalidator {
        type Error;
        type Snapshot;
        fn revalidate(&self, candidate: &CandidateIntent) -> Result<Self::Snapshot, Self::Error>;
    }

    pub trait CandidateEvaluator<Snapshot> {
        type Error;
        type Evaluation;
        fn evaluate(&self, snapshot: &Snapshot) -> Result<Self::Evaluation, Self::Error>;
    }

    pub trait PolicyGate<Evaluation> {
        type Error;
        fn authorize(&self, evaluation: &Evaluation) -> Result<(), Self::Error>;
    }

    pub trait TransactionSimulator<Intent> {
        type Error;
        type Simulation;
        fn simulate(&self, intent: &Intent) -> Result<Self::Simulation, Self::Error>;
    }

    pub trait LeaseStore {
        type Error;
        fn acquire(&self, conflict_key: &str, ttl_seconds: u64) -> Result<bool, Self::Error>;
        fn release(&self, conflict_key: &str) -> Result<(), Self::Error>;
    }

    pub trait TransactionSigner<Intent> {
        type Error;
        type SignedTransaction;
        fn sign(&self, intent: &Intent) -> Result<Self::SignedTransaction, Self::Error>;
    }

    pub trait TransactionSubmitter<SignedTransaction> {
        type Error;
        fn submit(&self, transaction: &SignedTransaction) -> Result<String, Self::Error>;
    }

    pub trait TransactionConfirmer {
        type Error;
        fn confirmed_slot(&self, signature: &str) -> Result<Option<u64>, Self::Error>;
    }

    pub trait PostconditionVerifier {
        type Error;
        fn verify(
            &self,
            candidate: &CandidateIntent,
            confirmed_slot: u64,
        ) -> Result<(), Self::Error>;
    }

    pub trait AttemptStore {
        type Error;
        fn create_or_load(
            &self,
            work_key: &str,
            candidate: &CandidateIntent,
        ) -> Result<String, Self::Error>;
        fn append_event(
            &self,
            attempt_id: &str,
            expected_sequence: u64,
            event: &LifecycleEvent,
        ) -> Result<(), Self::Error>;
        fn journal_outcome(&self, outcome: &ExecutionOutcome) -> Result<(), Self::Error>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureBundle {
        cases: Vec<FixtureCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureCase {
        name: String,
        locked_conflict_keys: Vec<String>,
        candidates: Vec<CandidateIntent>,
        expected_order: Vec<String>,
    }

    #[test]
    fn shared_scheduler_fixtures_match() {
        let fixtures: FixtureBundle = serde_json::from_str(include_str!(
            "../../../../fixtures/conformance/v1/scheduler-cases.json"
        ))
        .expect("fixtures must parse");

        for fixture in fixtures.cases {
            let mut scheduler = Scheduler::new("local-snapshot-0");
            for key in fixture.locked_conflict_keys {
                scheduler.mark_in_flight(key);
            }
            for candidate in fixture.candidates {
                scheduler.enqueue(candidate);
            }
            let mut actual = Vec::new();
            while let Some(candidate) = scheduler.pop_next() {
                actual.push(candidate.candidate_id);
            }
            assert_eq!(actual, fixture.expected_order, "{}", fixture.name);
        }
    }

    #[test]
    fn rejects_candidates_from_another_protocol_revision() {
        let mut scheduler = Scheduler::new("local-snapshot-0");
        let candidate = CandidateIntent {
            candidate_id: "candidate-a".into(),
            job_kind: JobKind::TakeProfit,
            conflict_key: "position:a".into(),
            market: "market:a".into(),
            target: "position:a".into(),
            observed_slot: 1,
            protocol_revision: "next-revision".into(),
            expected_state_hash: "a".repeat(64),
        };
        assert_eq!(
            scheduler.enqueue(candidate),
            EnqueueResult::RevisionMismatch
        );
    }
}
