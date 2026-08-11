import type {
  CandidateIntent,
  ExecutionOutcome,
  ExpectedRace,
  OutcomeStatus,
  ReasonCode,
} from "./index.ts";

export type ExecutionStep =
  | "discover"
  | "revalidate"
  | "evaluate"
  | "policy"
  | "simulate"
  | "lease"
  | "sign"
  | "submit"
  | "confirm"
  | "postcondition"
  | "journal";

export type AttemptPhase =
  | "active"
  | "awaiting_reconciliation"
  | "outcome_pending_journal"
  | "complete";

export type FailureClass = "retryable" | "terminal";
export type BlockhashExpiryTiming = "before_submit" | "after_submit_unknown";

export type LifecycleEvent =
  | {
      readonly type: "step_succeeded";
      readonly step: ExecutionStep;
      readonly blockhash?: string;
      readonly lastValidBlockHeight?: number;
      readonly signature?: string;
      readonly slot?: number;
    }
  | {
      readonly type: "expected_race";
      readonly step: ExecutionStep;
      readonly raceCode: ExpectedRace;
    }
  | {
      readonly type: "failure";
      readonly step: ExecutionStep;
      readonly failureClass: FailureClass;
      readonly reasonCode: ReasonCode;
    }
  | { readonly type: "blockhash_expired"; readonly timing: BlockhashExpiryTiming }
  | { readonly type: "submitted_not_landed_finalized" }
  | { readonly type: "submitted_landed"; readonly slot: number };

export interface AttemptState {
  readonly schemaVersion: 1;
  readonly attemptId: string;
  readonly attemptNumber: number;
  readonly workKey: string;
  readonly candidate: CandidateIntent;
  readonly phase: AttemptPhase;
  readonly nextStep: ExecutionStep | null;
  readonly eventSequence: number;
  readonly signingGeneration: number;
  readonly blockhash: string | null;
  readonly lastValidBlockHeight: number | null;
  readonly signature: string | null;
  readonly confirmedSlot: number | null;
  readonly outcome: ExecutionOutcome | null;
  readonly createdAt: string | null;
  readonly updatedAt: string | null;
}

export type TransitionErrorCode =
  | "invalid_phase"
  | "unexpected_step"
  | "missing_signing_metadata"
  | "missing_confirmation_slot"
  | "missing_signature"
  | "outcome_missing";

export class TransitionError extends Error {
  readonly code: TransitionErrorCode;

  constructor(code: TransitionErrorCode, message: string) {
    super(message);
    this.name = "TransitionError";
    this.code = code;
  }
}

export function createAttemptState(
  attemptId: string,
  workKey: string,
  candidate: CandidateIntent,
): AttemptState {
  return {
    schemaVersion: 1,
    attemptId,
    attemptNumber: 1,
    workKey,
    candidate,
    phase: "active",
    nextStep: "discover",
    eventSequence: 0,
    signingGeneration: 0,
    blockhash: null,
    lastValidBlockHeight: null,
    signature: null,
    confirmedSlot: null,
    outcome: null,
    createdAt: null,
    updatedAt: null,
  };
}

/** Returns a new state. A rejected event never mutates the supplied state. */
export function applyLifecycleEvent(state: AttemptState, event: LifecycleEvent): AttemptState {
  const next: MutableAttemptState = { ...state };
  reduce(next, event);
  next.eventSequence += 1;
  return next;
}

type MutableAttemptState = {
  -readonly [Key in keyof AttemptState]: AttemptState[Key];
};

function reduce(state: MutableAttemptState, event: LifecycleEvent): void {
  switch (event.type) {
    case "step_succeeded":
      stepSucceeded(state, event);
      return;
    case "expected_race":
      requireActive(state);
      requireStep(state, event.step);
      state.outcome = outcome(state, "skipped", raceReason(event.raceCode), event.raceCode);
      state.phase = "outcome_pending_journal";
      state.nextStep = "journal";
      return;
    case "failure":
      if (state.phase !== "active" && state.phase !== "awaiting_reconciliation") {
        throw invalidPhase(state.phase);
      }
      requireStep(state, event.step);
      state.outcome = outcome(
        state,
        event.failureClass === "retryable" ? "retryable_failure" : "terminal_failure",
        event.reasonCode,
        null,
      );
      state.phase = "outcome_pending_journal";
      state.nextStep = "journal";
      return;
    case "blockhash_expired":
      blockhashExpired(state, event.timing);
      return;
    case "submitted_not_landed_finalized":
      if (state.phase !== "awaiting_reconciliation") throw invalidPhase(state.phase);
      startNewSigningGeneration(state);
      return;
    case "submitted_landed":
      if (state.phase !== "awaiting_reconciliation") throw invalidPhase(state.phase);
      if (!state.signature) {
        throw new TransitionError(
          "missing_signature",
          "submitted transaction has no recorded signature",
        );
      }
      state.confirmedSlot = event.slot;
      state.phase = "active";
      state.nextStep = "postcondition";
  }
}

function stepSucceeded(
  state: MutableAttemptState,
  event: Extract<LifecycleEvent, { type: "step_succeeded" }>,
): void {
  if (!state.nextStep) {
    throw new TransitionError("invalid_phase", "complete attempt has no next step");
  }
  if (event.step !== state.nextStep) throw unexpectedStep(state.nextStep, event.step);
  if (state.phase === "complete") throw invalidPhase(state.phase);
  if (state.phase === "outcome_pending_journal" && event.step !== "journal") {
    throw unexpectedStep("journal", event.step);
  }
  if (state.phase === "awaiting_reconciliation" && event.step !== "confirm") {
    throw unexpectedStep("confirm", event.step);
  }

  switch (event.step) {
    case "sign":
      if (
        !event.blockhash ||
        event.lastValidBlockHeight === undefined ||
        !event.signature
      ) {
        throw new TransitionError(
          "missing_signing_metadata",
          "sign requires blockhash, lastValidBlockHeight, and signature",
        );
      }
      state.blockhash = event.blockhash;
      state.lastValidBlockHeight = event.lastValidBlockHeight;
      state.signature = event.signature;
      state.nextStep = "submit";
      return;
    case "confirm":
      if (event.slot === undefined) {
        throw new TransitionError("missing_confirmation_slot", "confirm requires a slot");
      }
      state.confirmedSlot = event.slot;
      state.phase = "active";
      state.nextStep = "postcondition";
      return;
    case "postcondition":
      if (!state.signature) {
        throw new TransitionError(
          "missing_signature",
          "postcondition requires a recorded signature",
        );
      }
      if (state.confirmedSlot === null) {
        throw new TransitionError(
          "missing_confirmation_slot",
          "postcondition requires a confirmed slot",
        );
      }
      state.outcome = outcome(state, "executed", "confirmed", null);
      state.phase = "outcome_pending_journal";
      state.nextStep = "journal";
      return;
    case "journal":
      if (state.phase !== "outcome_pending_journal" || !state.outcome) {
        throw new TransitionError("outcome_missing", "journal requires a completed outcome");
      }
      state.phase = "complete";
      state.nextStep = null;
      return;
    default:
      requireActive(state);
      state.nextStep = nextExecutionStep(event.step);
  }
}

function blockhashExpired(state: MutableAttemptState, timing: BlockhashExpiryTiming): void {
  if (timing === "before_submit") {
    if (state.phase !== "active" || state.nextStep !== "submit") {
      throw unexpectedStep("submit", state.nextStep ?? "discover");
    }
    startNewSigningGeneration(state);
    return;
  }
  if (state.phase !== "active" || state.nextStep !== "confirm") {
    throw unexpectedStep("confirm", state.nextStep ?? "discover");
  }
  state.phase = "awaiting_reconciliation";
  state.nextStep = "confirm";
}

function startNewSigningGeneration(state: MutableAttemptState): void {
  state.signingGeneration += 1;
  state.blockhash = null;
  state.lastValidBlockHeight = null;
  state.signature = null;
  state.confirmedSlot = null;
  state.outcome = null;
  state.phase = "active";
  state.nextStep = "revalidate";
}

function outcome(
  state: AttemptState,
  status: OutcomeStatus,
  reasonCode: ReasonCode,
  raceCode: ExpectedRace | null,
): ExecutionOutcome {
  return {
    attemptId: state.attemptId,
    attemptNumber: state.attemptNumber,
    workKey: state.workKey,
    candidateId: state.candidate.candidateId,
    jobKind: state.candidate.jobKind,
    conflictKey: state.candidate.conflictKey,
    status,
    reasonCode,
    raceCode,
    observedSlot: state.candidate.observedSlot,
    finalizedSlot: state.confirmedSlot,
    protocolRevision: state.candidate.protocolRevision,
    signature: state.signature,
    signingGeneration: state.signingGeneration,
  };
}

function raceReason(race: ExpectedRace): ReasonCode {
  switch (race) {
    case "account_changed":
      return "state_changed";
    case "trigger_no_longer_met":
      return "bounds_not_met";
    case "lease_contended":
      return "lease_not_acquired";
    default:
      return "already_resolved";
  }
}

function nextExecutionStep(step: ExecutionStep): ExecutionStep | null {
  const index = executionSteps.indexOf(step);
  return executionSteps[index + 1] ?? null;
}

const executionSteps: readonly ExecutionStep[] = [
  "discover",
  "revalidate",
  "evaluate",
  "policy",
  "simulate",
  "lease",
  "sign",
  "submit",
  "confirm",
  "postcondition",
  "journal",
];

function requireActive(state: AttemptState): void {
  if (state.phase !== "active") throw invalidPhase(state.phase);
}

function requireStep(state: AttemptState, actual: ExecutionStep): void {
  if (!state.nextStep) {
    throw new TransitionError("invalid_phase", "complete attempt has no next step");
  }
  if (actual !== state.nextStep) throw unexpectedStep(state.nextStep, actual);
}

function invalidPhase(phase: AttemptPhase): TransitionError {
  return new TransitionError("invalid_phase", `event is invalid in phase ${phase}`);
}

function unexpectedStep(expected: ExecutionStep, actual: ExecutionStep): TransitionError {
  return new TransitionError("unexpected_step", `expected ${expected}, got ${actual}`);
}
