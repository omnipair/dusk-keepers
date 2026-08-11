import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  EnvelopeValidationError,
  EnvelopeValidator,
  parseProtocolLock,
  type EnvelopeValidationErrorCode,
  type InstructionContract,
  type JobEnvelope,
} from "../packages/dusk-adapter/src/index.ts";

interface FixtureBundle {
  readonly validCases: readonly { readonly name: string; readonly envelope: JobEnvelope }[];
  readonly invalidCases: readonly {
    readonly name: string;
    readonly expectedError: EnvelopeValidationErrorCode;
    readonly envelope: JobEnvelope;
  }[];
}

const lock = parseProtocolLock(
  JSON.parse(await readFile(new URL("../../protocol.lock.json", import.meta.url), "utf8")) as unknown,
);
const contract = JSON.parse(
  await readFile(new URL("../../protocol/keeper-instructions.v1.json", import.meta.url), "utf8"),
) as InstructionContract;
const fixtures = JSON.parse(
  await readFile(
    new URL("../../fixtures/conformance/v1/instruction-envelope-cases.json", import.meta.url),
    "utf8",
  ),
) as FixtureBundle;

const validator = new EnvelopeValidator(lock, contract);

for (const fixture of fixtures.validCases) {
  test(`real instruction envelope: ${fixture.name}`, () => {
    const validated = validator.validate(fixture.envelope);
    assert.equal(validated.envelope.paritySha256, fixture.envelope.paritySha256);
  });
}

test("captured lock cannot produce a signable instruction envelope", () => {
  const fixture = fixtures.validCases[0];
  assert.ok(fixture, "a valid instruction fixture must exist");
  assert.throws(
    () => validator.validateForSigning(fixture.envelope),
    (error) =>
      error instanceof EnvelopeValidationError && error.code === "live_protocol_not_ready",
  );
});

for (const fixture of fixtures.invalidCases) {
  test(`rejects instruction envelope: ${fixture.name}`, () => {
    assert.throws(
      () => validator.validate(fixture.envelope),
      (error) => error instanceof EnvelopeValidationError && error.code === fixture.expectedError,
    );
  });
}
