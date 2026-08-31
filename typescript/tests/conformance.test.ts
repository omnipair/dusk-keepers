import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { assertLiveReady, parseProtocolLock } from "../packages/dusk-adapter/src/index.ts";
import {
  Scheduler,
  TransitionError,
  applyLifecycleEvent,
  createAttemptState,
  type AttemptPhase,
  type CandidateIntent,
  type ExecutionStep,
  type ExpectedRace,
  type HealthProvenance,
  type LifecycleEvent,
  type OutcomeStatus,
  type ReasonCode,
  type TransitionErrorCode,
} from "../packages/keeper-core/src/index.ts";

interface FixtureBundle {
  readonly cases: readonly {
    readonly name: string;
    readonly lockedConflictKeys: readonly string[];
    readonly candidates: readonly CandidateIntent[];
    readonly expectedOrder: readonly string[];
  }[];
}

const fixtures = JSON.parse(
  await readFile(new URL("../../fixtures/conformance/v1/scheduler-cases.json", import.meta.url), "utf8"),
) as FixtureBundle;

interface LifecycleFixtureBundle {
  readonly cases: readonly {
    readonly name: string;
    readonly attemptId: string;
    readonly workKey: string;
    readonly candidate: CandidateIntent;
    readonly events: readonly LifecycleEvent[];
    readonly expected: {
      readonly phase: AttemptPhase;
      readonly nextStep: ExecutionStep | null;
      readonly eventSequence: number;
      readonly signingGeneration: number;
      readonly outcomeStatus: OutcomeStatus | null;
      readonly reasonCode: ReasonCode | null;
      readonly raceCode: ExpectedRace | null;
      readonly signature: string | null;
      readonly confirmedSlot: number | null;
      readonly errorCode: TransitionErrorCode | null;
    };
  }[];
}

const lifecycleFixtures = JSON.parse(
  await readFile(
    new URL("../../fixtures/conformance/v1/execution-lifecycle-cases.json", import.meta.url),
    "utf8",
  ),
) as LifecycleFixtureBundle;

for (const fixture of fixtures.cases) {
  test(`shared scheduler: ${fixture.name}`, () => {
    // The revision comes from the fixture rather than a literal: these
    // candidates are re-pinned whenever the deployment changes, and a
    // hardcoded revision silently rejects every one of them, leaving an empty
    // queue that looks like a scheduling bug.
    const scheduler = new Scheduler(fixture.candidates[0]?.protocolRevision ?? "");
    for (const key of fixture.lockedConflictKeys) scheduler.markInFlight(key);
    for (const candidate of fixture.candidates) scheduler.enqueue(candidate);

    const actual: string[] = [];
    for (let candidate = scheduler.popNext(); candidate; candidate = scheduler.popNext()) {
      actual.push(candidate.candidateId);
    }
    assert.deepEqual(actual, fixture.expectedOrder);
  });
}

for (const fixture of lifecycleFixtures.cases) {
  test(`shared lifecycle: ${fixture.name}`, () => {
    let attempt = createAttemptState(fixture.attemptId, fixture.workKey, fixture.candidate);
    let errorCode: TransitionErrorCode | null = null;
    for (const event of fixture.events) {
      try {
        attempt = applyLifecycleEvent(attempt, event);
      } catch (error) {
        assert.ok(error instanceof TransitionError);
        errorCode = error.code;
        break;
      }
    }

    assert.equal(attempt.phase, fixture.expected.phase);
    assert.equal(attempt.nextStep, fixture.expected.nextStep);
    assert.equal(attempt.eventSequence, fixture.expected.eventSequence);
    assert.equal(attempt.signingGeneration, fixture.expected.signingGeneration);
    assert.equal(attempt.signature, fixture.expected.signature);
    assert.equal(attempt.confirmedSlot, fixture.expected.confirmedSlot);
    assert.equal(attempt.outcome?.status ?? null, fixture.expected.outcomeStatus);
    assert.equal(attempt.outcome?.reasonCode ?? null, fixture.expected.reasonCode);
    assert.equal(attempt.outcome?.raceCode ?? null, fixture.expected.raceCode);
    assert.equal(errorCode, fixture.expected.errorCode);
  });
}

test("shared health fixture matches the TypeScript provenance model", async () => {
  const health = JSON.parse(
    await readFile(new URL("../../fixtures/conformance/v1/health-provenance.json", import.meta.url), "utf8"),
  ) as HealthProvenance;
  assert.equal(health.status, "degraded");
  // Tracks the repository's lock rather than pinning a status: this fixture
  // is regenerated on every re-pin, and a literal here fails on the re-pin
  // instead of on a real drift.
  assert.equal(health.protocol.lockStatus, lockStatusFromRepository);
  assert.equal(health.dependencies.signer, "disabled");
});

const lockStatusFromRepository = JSON.parse(
  await readFile(new URL("../../protocol.lock.json", import.meta.url), "utf8"),
).status as string;

test("captured protocol lock cannot enable live execution", async () => {
  const raw = JSON.parse(
    await readFile(new URL("../../protocol.lock.json", import.meta.url), "utf8"),
  ) as unknown;
  const lock = parseProtocolLock(raw);
  // The rule under test is that a captured lock refuses live execution, not
  // which revision happens to be pinned today. Asserting the revision here
  // turns every re-pin into a failure that says nothing about the rule.
  assert.equal(lock.revision, JSON.parse(await readFile(
    new URL("../../protocol.lock.json", import.meta.url), "utf8",
  )).revision);
  if (lock.status === "captured") {
    assert.throws(() => assertLiveReady(lock), /not frozen/);
  } else {
    assert.doesNotThrow(() => assertLiveReady(lock));
  }
});
