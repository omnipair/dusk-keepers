export type JobKind =
  | "leverage_liquidation"
  | "lending_liquidation_trigger"
  | "lending_liquidation_bid"
  | "lending_liquidation_settle"
  | "stop_loss"
  | "take_profit"
  | "auction_bid"
  | "lifecycle"
  | "sentinel";

export interface CandidateIntent {
  readonly candidateId: string;
  readonly jobKind: JobKind;
  readonly conflictKey: string;
  readonly market: string;
  readonly target: string;
  readonly observedSlot: number;
  readonly protocolRevision: string;
  readonly expectedStateHash: string;
}

export type EnqueueResult =
  | "accepted"
  | "superseded"
  | "conflict_locked"
  | "revision_mismatch";

const priorities: Readonly<Record<JobKind, number>> = {
  leverage_liquidation: 500,
  lending_liquidation_trigger: 490,
  lending_liquidation_bid: 200,
  lending_liquidation_settle: 500,
  stop_loss: 400,
  take_profit: 300,
  auction_bid: 200,
  lifecycle: 100,
  sentinel: 0,
};

export class Scheduler {
  readonly #protocolRevision: string;
  readonly #pending = new Map<string, CandidateIntent>();
  readonly #inFlight = new Set<string>();

  constructor(protocolRevision: string) {
    this.#protocolRevision = protocolRevision;
  }

  enqueue(candidate: CandidateIntent): EnqueueResult {
    if (candidate.protocolRevision !== this.#protocolRevision) return "revision_mismatch";
    if (this.#inFlight.has(candidate.conflictKey)) return "conflict_locked";

    const current = this.#pending.get(candidate.conflictKey);
    if (current && compareCandidates(candidate, current) <= 0) return "superseded";
    this.#pending.set(candidate.conflictKey, candidate);
    return "accepted";
  }

  popNext(): CandidateIntent | undefined {
    let selected: CandidateIntent | undefined;
    for (const candidate of this.#pending.values()) {
      if (!selected || compareCandidates(candidate, selected) > 0) selected = candidate;
    }
    if (!selected) return undefined;
    this.#pending.delete(selected.conflictKey);
    this.#inFlight.add(selected.conflictKey);
    return selected;
  }

  markInFlight(conflictKey: string): void {
    this.#pending.delete(conflictKey);
    this.#inFlight.add(conflictKey);
  }

  finish(conflictKey: string): boolean {
    return this.#inFlight.delete(conflictKey);
  }

  get pendingCount(): number {
    return this.#pending.size;
  }
}

function compareCandidates(left: CandidateIntent, right: CandidateIntent): number {
  return (
    priorities[left.jobKind] - priorities[right.jobKind] ||
    left.observedSlot - right.observedSlot ||
    left.candidateId.localeCompare(right.candidateId)
  );
}

export type OutcomeStatus = "executed" | "skipped" | "retryable_failure" | "terminal_failure";

export type ReasonCode =
  | "confirmed"
  | "shadow_mode"
  | "stale_observation"
  | "state_changed"
  | "already_resolved"
  | "bounds_not_met"
  | "policy_denied"
  | "lease_not_acquired"
  | "simulation_rejected"
  | "signing_denied"
  | "rpc_unavailable"
  | "blockhash_expired"
  | "confirmation_unknown"
  | "postcondition_failed"
  | "insufficient_signer_balance"
  | "protocol_revision_mismatch"
  | "unsupported_protocol"
  | "unknown";

export type ExpectedRace =
  | "account_changed"
  | "account_closed"
  | "position_already_closed"
  | "order_already_closed"
  | "trigger_no_longer_met"
  | "obligation_no_longer_liquidatable"
  | "auction_already_settled"
  | "lease_contended"
  | "duplicate_signature_confirmed";

export interface ExecutionOutcome {
  readonly attemptId: string;
  readonly attemptNumber: number;
  readonly workKey: string;
  readonly candidateId: string;
  readonly jobKind: JobKind;
  readonly conflictKey: string;
  readonly status: OutcomeStatus;
  readonly reasonCode: ReasonCode;
  readonly raceCode: ExpectedRace | null;
  readonly observedSlot: number;
  readonly finalizedSlot: number | null;
  readonly protocolRevision: string;
  readonly signature: string | null;
  readonly signingGeneration: number;
}

export interface CandidateSource {
  discover(): Promise<readonly CandidateIntent[]>;
}

export interface StateRevalidator<Snapshot> {
  revalidate(candidate: CandidateIntent): Promise<Snapshot>;
}

export interface CandidateEvaluator<Snapshot, Evaluation> {
  evaluate(snapshot: Snapshot): Promise<Evaluation>;
}

export interface PolicyGate<Evaluation> {
  authorize(evaluation: Evaluation): Promise<void>;
}

export interface TransactionSimulator<Intent, Simulation> {
  simulate(intent: Intent): Promise<Simulation>;
}

export interface LeaseStore {
  acquire(conflictKey: string, ttlSeconds: number): Promise<boolean>;
  release(conflictKey: string): Promise<void>;
}

export interface TransactionSigner<Intent, SignedTransaction> {
  sign(intent: Intent): Promise<SignedTransaction>;
}

export interface TransactionSubmitter<SignedTransaction> {
  submit(transaction: SignedTransaction): Promise<string>;
}

export interface TransactionConfirmer {
  confirmedSlot(signature: string): Promise<number | null>;
}

export interface PostconditionVerifier {
  verify(candidate: CandidateIntent, confirmedSlot: number): Promise<void>;
}

export interface AttemptStore {
  createOrLoad(workKey: string, candidate: CandidateIntent): Promise<string>;
  appendEvent(
    attemptId: string,
    expectedSequence: number,
    event: import("./lifecycle.ts").LifecycleEvent,
  ): Promise<void>;
  journalOutcome(outcome: ExecutionOutcome): Promise<void>;
}

export * from "./health.ts";
export * from "./lifecycle.ts";
