//! Pure execution lifecycle reducer.
//!
//! Chain I/O lives behind ports in the crate root. This reducer only accepts
//! durable facts and therefore produces the same state in Rust and TypeScript.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{CandidateIntent, ExecutionOutcome, OutcomeStatus, ReasonCode};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStep {
    Discover,
    Revalidate,
    Evaluate,
    Policy,
    Simulate,
    Lease,
    Sign,
    Submit,
    Confirm,
    Postcondition,
    Journal,
}

impl ExecutionStep {
    const fn next(self) -> Option<Self> {
        match self {
            Self::Discover => Some(Self::Revalidate),
            Self::Revalidate => Some(Self::Evaluate),
            Self::Evaluate => Some(Self::Policy),
            Self::Policy => Some(Self::Simulate),
            Self::Simulate => Some(Self::Lease),
            Self::Lease => Some(Self::Sign),
            Self::Sign => Some(Self::Submit),
            Self::Submit => Some(Self::Confirm),
            Self::Confirm => Some(Self::Postcondition),
            Self::Postcondition => Some(Self::Journal),
            Self::Journal => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    Active,
    AwaitingReconciliation,
    OutcomePendingJournal,
    Complete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedRace {
    AccountChanged,
    AccountClosed,
    PositionAlreadyClosed,
    OrderAlreadyClosed,
    TriggerNoLongerMet,
    ObligationNoLongerLiquidatable,
    AuctionAlreadySettled,
    LeaseContended,
    DuplicateSignatureConfirmed,
}

impl ExpectedRace {
    const fn reason_code(self) -> ReasonCode {
        match self {
            Self::AccountChanged => ReasonCode::StateChanged,
            Self::AccountClosed
            | Self::PositionAlreadyClosed
            | Self::OrderAlreadyClosed
            | Self::ObligationNoLongerLiquidatable
            | Self::AuctionAlreadySettled
            | Self::DuplicateSignatureConfirmed => ReasonCode::AlreadyResolved,
            Self::TriggerNoLongerMet => ReasonCode::BoundsNotMet,
            Self::LeaseContended => ReasonCode::LeaseNotAcquired,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Retryable,
    Terminal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockhashExpiryTiming {
    BeforeSubmit,
    AfterSubmitUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LifecycleEvent {
    StepSucceeded {
        step: ExecutionStep,
        #[serde(default)]
        blockhash: Option<String>,
        #[serde(default)]
        last_valid_block_height: Option<u64>,
        #[serde(default)]
        signature: Option<String>,
        #[serde(default)]
        slot: Option<u64>,
    },
    ExpectedRace {
        step: ExecutionStep,
        race_code: ExpectedRace,
    },
    Failure {
        step: ExecutionStep,
        failure_class: FailureClass,
        reason_code: ReasonCode,
    },
    BlockhashExpired {
        timing: BlockhashExpiryTiming,
    },
    SubmittedNotLandedFinalized,
    SubmittedLanded {
        slot: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptState {
    pub schema_version: u8,
    pub attempt_id: String,
    pub attempt_number: u32,
    pub work_key: String,
    pub candidate: CandidateIntent,
    pub phase: AttemptPhase,
    pub next_step: Option<ExecutionStep>,
    pub event_sequence: u64,
    pub signing_generation: u32,
    pub blockhash: Option<String>,
    pub last_valid_block_height: Option<u64>,
    pub signature: Option<String>,
    pub confirmed_slot: Option<u64>,
    pub outcome: Option<ExecutionOutcome>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl AttemptState {
    pub fn new(
        attempt_id: impl Into<String>,
        work_key: impl Into<String>,
        candidate: CandidateIntent,
    ) -> Self {
        Self {
            schema_version: 1,
            attempt_id: attempt_id.into(),
            attempt_number: 1,
            work_key: work_key.into(),
            candidate,
            phase: AttemptPhase::Active,
            next_step: Some(ExecutionStep::Discover),
            event_sequence: 0,
            signing_generation: 0,
            blockhash: None,
            last_valid_block_height: None,
            signature: None,
            confirmed_slot: None,
            outcome: None,
            created_at: None,
            updated_at: None,
        }
    }

    /// Applies one already-observed fact. Errors never mutate the state.
    pub fn apply(&mut self, event: LifecycleEvent) -> Result<(), TransitionError> {
        let mut next = self.clone();
        next.reduce(event)?;
        next.event_sequence += 1;
        *self = next;
        Ok(())
    }

    fn reduce(&mut self, event: LifecycleEvent) -> Result<(), TransitionError> {
        match event {
            LifecycleEvent::StepSucceeded {
                step,
                blockhash,
                last_valid_block_height,
                signature,
                slot,
            } => self.step_succeeded(step, blockhash, last_valid_block_height, signature, slot),
            LifecycleEvent::ExpectedRace { step, race_code } => {
                self.require_active()?;
                self.require_step(step)?;
                self.outcome = Some(self.outcome(
                    OutcomeStatus::Skipped,
                    race_code.reason_code(),
                    Some(race_code),
                ));
                self.phase = AttemptPhase::OutcomePendingJournal;
                self.next_step = Some(ExecutionStep::Journal);
                Ok(())
            }
            LifecycleEvent::Failure {
                step,
                failure_class,
                reason_code,
            } => {
                if !matches!(
                    self.phase,
                    AttemptPhase::Active | AttemptPhase::AwaitingReconciliation
                ) {
                    return Err(TransitionError::invalid_phase(self.phase));
                }
                self.require_step(step)?;
                let status = match failure_class {
                    FailureClass::Retryable => OutcomeStatus::RetryableFailure,
                    FailureClass::Terminal => OutcomeStatus::TerminalFailure,
                };
                self.outcome = Some(self.outcome(status, reason_code, None));
                self.phase = AttemptPhase::OutcomePendingJournal;
                self.next_step = Some(ExecutionStep::Journal);
                Ok(())
            }
            LifecycleEvent::BlockhashExpired { timing } => self.blockhash_expired(timing),
            LifecycleEvent::SubmittedNotLandedFinalized => {
                if self.phase != AttemptPhase::AwaitingReconciliation {
                    return Err(TransitionError::invalid_phase(self.phase));
                }
                self.start_new_signing_generation();
                Ok(())
            }
            LifecycleEvent::SubmittedLanded { slot } => {
                if self.phase != AttemptPhase::AwaitingReconciliation {
                    return Err(TransitionError::invalid_phase(self.phase));
                }
                if self.signature.is_none() {
                    return Err(TransitionError::new(
                        TransitionErrorCode::MissingSignature,
                        "submitted transaction has no recorded signature",
                    ));
                }
                self.confirmed_slot = Some(slot);
                self.phase = AttemptPhase::Active;
                self.next_step = Some(ExecutionStep::Postcondition);
                Ok(())
            }
        }
    }

    fn step_succeeded(
        &mut self,
        step: ExecutionStep,
        blockhash: Option<String>,
        last_valid_block_height: Option<u64>,
        signature: Option<String>,
        slot: Option<u64>,
    ) -> Result<(), TransitionError> {
        let expected = self.next_step.ok_or_else(|| {
            TransitionError::new(
                TransitionErrorCode::InvalidPhase,
                "complete attempt has no next step",
            )
        })?;
        if step != expected {
            return Err(TransitionError::unexpected_step(expected, step));
        }

        match self.phase {
            AttemptPhase::Complete => return Err(TransitionError::invalid_phase(self.phase)),
            AttemptPhase::OutcomePendingJournal if step != ExecutionStep::Journal => {
                return Err(TransitionError::unexpected_step(
                    ExecutionStep::Journal,
                    step,
                ));
            }
            AttemptPhase::AwaitingReconciliation if step != ExecutionStep::Confirm => {
                return Err(TransitionError::unexpected_step(
                    ExecutionStep::Confirm,
                    step,
                ));
            }
            AttemptPhase::Active | AttemptPhase::OutcomePendingJournal => {}
            AttemptPhase::AwaitingReconciliation => {}
        }

        match step {
            ExecutionStep::Sign => {
                let (Some(blockhash), Some(last_valid_block_height), Some(signature)) =
                    (blockhash, last_valid_block_height, signature)
                else {
                    return Err(TransitionError::new(
                        TransitionErrorCode::MissingSigningMetadata,
                        "sign requires blockhash, lastValidBlockHeight, and signature",
                    ));
                };
                self.blockhash = Some(blockhash);
                self.last_valid_block_height = Some(last_valid_block_height);
                self.signature = Some(signature);
                self.next_step = Some(ExecutionStep::Submit);
            }
            ExecutionStep::Confirm => {
                let slot = slot.ok_or_else(|| {
                    TransitionError::new(
                        TransitionErrorCode::MissingConfirmationSlot,
                        "confirm requires a slot",
                    )
                })?;
                self.confirmed_slot = Some(slot);
                self.phase = AttemptPhase::Active;
                self.next_step = Some(ExecutionStep::Postcondition);
            }
            ExecutionStep::Postcondition => {
                if self.signature.is_none() {
                    return Err(TransitionError::new(
                        TransitionErrorCode::MissingSignature,
                        "postcondition requires a recorded signature",
                    ));
                }
                if self.confirmed_slot.is_none() {
                    return Err(TransitionError::new(
                        TransitionErrorCode::MissingConfirmationSlot,
                        "postcondition requires a confirmed slot",
                    ));
                }
                self.outcome =
                    Some(self.outcome(OutcomeStatus::Executed, ReasonCode::Confirmed, None));
                self.phase = AttemptPhase::OutcomePendingJournal;
                self.next_step = Some(ExecutionStep::Journal);
            }
            ExecutionStep::Journal => {
                if self.phase != AttemptPhase::OutcomePendingJournal || self.outcome.is_none() {
                    return Err(TransitionError::new(
                        TransitionErrorCode::OutcomeMissing,
                        "journal requires a completed outcome",
                    ));
                }
                self.phase = AttemptPhase::Complete;
                self.next_step = None;
            }
            _ => {
                self.require_active()?;
                self.next_step = step.next();
            }
        }
        Ok(())
    }

    fn blockhash_expired(&mut self, timing: BlockhashExpiryTiming) -> Result<(), TransitionError> {
        match timing {
            BlockhashExpiryTiming::BeforeSubmit => {
                if self.phase != AttemptPhase::Active
                    || self.next_step != Some(ExecutionStep::Submit)
                {
                    return Err(TransitionError::unexpected_step(
                        ExecutionStep::Submit,
                        self.next_step.unwrap_or(ExecutionStep::Discover),
                    ));
                }
                self.start_new_signing_generation();
            }
            BlockhashExpiryTiming::AfterSubmitUnknown => {
                if self.phase != AttemptPhase::Active
                    || self.next_step != Some(ExecutionStep::Confirm)
                {
                    return Err(TransitionError::unexpected_step(
                        ExecutionStep::Confirm,
                        self.next_step.unwrap_or(ExecutionStep::Discover),
                    ));
                }
                self.phase = AttemptPhase::AwaitingReconciliation;
                self.next_step = Some(ExecutionStep::Confirm);
            }
        }
        Ok(())
    }

    fn start_new_signing_generation(&mut self) {
        self.signing_generation += 1;
        self.blockhash = None;
        self.last_valid_block_height = None;
        self.signature = None;
        self.confirmed_slot = None;
        self.outcome = None;
        self.phase = AttemptPhase::Active;
        self.next_step = Some(ExecutionStep::Revalidate);
    }

    fn require_active(&self) -> Result<(), TransitionError> {
        if self.phase == AttemptPhase::Active {
            Ok(())
        } else {
            Err(TransitionError::invalid_phase(self.phase))
        }
    }

    fn require_step(&self, actual: ExecutionStep) -> Result<(), TransitionError> {
        let expected = self.next_step.ok_or_else(|| {
            TransitionError::new(
                TransitionErrorCode::InvalidPhase,
                "complete attempt has no next step",
            )
        })?;
        if actual == expected {
            Ok(())
        } else {
            Err(TransitionError::unexpected_step(expected, actual))
        }
    }

    fn outcome(
        &self,
        status: OutcomeStatus,
        reason_code: ReasonCode,
        race_code: Option<ExpectedRace>,
    ) -> ExecutionOutcome {
        ExecutionOutcome {
            attempt_id: self.attempt_id.clone(),
            attempt_number: self.attempt_number,
            work_key: self.work_key.clone(),
            candidate_id: self.candidate.candidate_id.clone(),
            job_kind: self.candidate.job_kind,
            conflict_key: self.candidate.conflict_key.clone(),
            status,
            reason_code,
            race_code,
            observed_slot: self.candidate.observed_slot,
            finalized_slot: self.confirmed_slot,
            protocol_revision: self.candidate.protocol_revision.clone(),
            signature: self.signature.clone(),
            signing_generation: self.signing_generation,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionErrorCode {
    InvalidPhase,
    UnexpectedStep,
    MissingSigningMetadata,
    MissingConfirmationSlot,
    MissingSignature,
    OutcomeMissing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionError {
    pub code: TransitionErrorCode,
    message: String,
}

impl TransitionError {
    fn new(code: TransitionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn invalid_phase(phase: AttemptPhase) -> Self {
        Self::new(
            TransitionErrorCode::InvalidPhase,
            format!("event is invalid in phase {phase:?}"),
        )
    }

    fn unexpected_step(expected: ExecutionStep, actual: ExecutionStep) -> Self {
        Self::new(
            TransitionErrorCode::UnexpectedStep,
            format!("expected {expected:?}, got {actual:?}"),
        )
    }
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TransitionError {}

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
        attempt_id: String,
        work_key: String,
        candidate: CandidateIntent,
        events: Vec<LifecycleEvent>,
        expected: ExpectedState,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ExpectedState {
        phase: AttemptPhase,
        next_step: Option<ExecutionStep>,
        event_sequence: u64,
        signing_generation: u32,
        outcome_status: Option<OutcomeStatus>,
        reason_code: Option<ReasonCode>,
        race_code: Option<ExpectedRace>,
        signature: Option<String>,
        confirmed_slot: Option<u64>,
        error_code: Option<TransitionErrorCode>,
    }

    #[test]
    fn shared_lifecycle_fixtures_match() {
        let fixtures: FixtureBundle = serde_json::from_str(include_str!(
            "../../../../fixtures/conformance/v1/execution-lifecycle-cases.json"
        ))
        .expect("lifecycle fixtures must parse");

        for fixture in fixtures.cases {
            let mut attempt =
                AttemptState::new(fixture.attempt_id, fixture.work_key, fixture.candidate);
            let mut error_code = None;
            for event in fixture.events {
                if let Err(error) = attempt.apply(event) {
                    error_code = Some(error.code);
                    break;
                }
            }

            assert_eq!(attempt.phase, fixture.expected.phase, "{}", fixture.name);
            assert_eq!(
                attempt.next_step, fixture.expected.next_step,
                "{}",
                fixture.name
            );
            assert_eq!(
                attempt.event_sequence, fixture.expected.event_sequence,
                "{}",
                fixture.name
            );
            assert_eq!(
                attempt.signing_generation, fixture.expected.signing_generation,
                "{}",
                fixture.name
            );
            assert_eq!(
                attempt.signature, fixture.expected.signature,
                "{}",
                fixture.name
            );
            assert_eq!(
                attempt.confirmed_slot, fixture.expected.confirmed_slot,
                "{}",
                fixture.name
            );
            assert_eq!(error_code, fixture.expected.error_code, "{}", fixture.name);
            assert_eq!(
                attempt.outcome.as_ref().map(|outcome| outcome.status),
                fixture.expected.outcome_status,
                "{}",
                fixture.name
            );
            assert_eq!(
                attempt.outcome.as_ref().map(|outcome| outcome.reason_code),
                fixture.expected.reason_code,
                "{}",
                fixture.name
            );
            assert_eq!(
                attempt
                    .outcome
                    .as_ref()
                    .and_then(|outcome| outcome.race_code),
                fixture.expected.race_code,
                "{}",
                fixture.name
            );
        }
    }
}
