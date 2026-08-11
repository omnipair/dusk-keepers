import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  AccountResolutionError,
  DeterministicAccountResolver,
  InstructionEncodingError,
  encodeKeeperInstructionJson,
  parseProtocolLock,
  type AccountResolutionErrorCode,
  type AccountResolutionManifest,
  type InstructionContract,
  type InstructionEncodingErrorCode,
} from "../packages/dusk-adapter/src/index.ts";

interface EncodingCase {
  readonly name: string;
  readonly specificationKey: string;
  readonly arguments: Readonly<Record<string, unknown>>;
  readonly expectedDataHex: string;
}

interface InvalidEncodingCase {
  readonly name: string;
  readonly specificationKey: string;
  readonly arguments: Readonly<Record<string, unknown>>;
  readonly expectedError: InstructionEncodingErrorCode;
}

interface PdaCase {
  readonly name: string;
  readonly recipeKey: string;
  readonly inputs: Readonly<Record<string, string>>;
  readonly expectedAddress: string;
  readonly expectedBump: number;
}

interface InvalidPdaCase {
  readonly name: string;
  readonly recipeKey: string;
  readonly inputs: Readonly<Record<string, string>>;
  readonly expectedError: AccountResolutionErrorCode;
}

interface StaticAccountCase {
  readonly name: string;
  readonly instructionKey: string;
  readonly accountName: string;
  readonly expectedAddress: string;
}

interface FixtureBundle {
  readonly encodingCases: readonly EncodingCase[];
  readonly invalidEncodingCases: readonly InvalidEncodingCase[];
  readonly pdaCases: readonly PdaCase[];
  readonly invalidPdaCases: readonly InvalidPdaCase[];
  readonly staticAccountCases: readonly StaticAccountCase[];
}

const lock = parseProtocolLock(
  JSON.parse(await readFile(new URL("../../protocol.lock.json", import.meta.url), "utf8")) as unknown,
);
const contract = JSON.parse(
  await readFile(new URL("../../protocol/keeper-instructions.v1.json", import.meta.url), "utf8"),
) as InstructionContract;
const manifest = JSON.parse(
  await readFile(
    new URL("../../protocol/keeper-account-resolution.v1.json", import.meta.url),
    "utf8",
  ),
) as AccountResolutionManifest;
const fixtures = JSON.parse(
  await readFile(
    new URL("../../fixtures/conformance/v1/adapter-codec-cases.json", import.meta.url),
    "utf8",
  ),
) as FixtureBundle;
const resolver = new DeterministicAccountResolver(lock, contract, manifest);

for (const fixture of fixtures.encodingCases) {
  test(`native Borsh encoding: ${fixture.name}`, () => {
    const actual = encodeKeeperInstructionJson(
      contract,
      fixture.specificationKey,
      fixture.arguments,
    );
    assert.equal(Buffer.from(actual).toString("hex"), fixture.expectedDataHex);
  });
}

for (const fixture of fixtures.invalidEncodingCases) {
  test(`rejects instruction arguments: ${fixture.name}`, () => {
    assert.throws(
      () =>
        encodeKeeperInstructionJson(contract, fixture.specificationKey, fixture.arguments),
      (error) =>
        error instanceof InstructionEncodingError && error.code === fixture.expectedError,
    );
  });
}

for (const fixture of fixtures.pdaCases) {
  test(`deterministic PDA: ${fixture.name}`, () => {
    assert.deepEqual(resolver.derivePdaStrings(fixture.recipeKey, fixture.inputs), {
      address: fixture.expectedAddress,
      bump: fixture.expectedBump,
    });
  });
}

for (const fixture of fixtures.invalidPdaCases) {
  test(`rejects PDA input: ${fixture.name}`, () => {
    assert.throws(
      () => resolver.derivePdaStrings(fixture.recipeKey, fixture.inputs),
      (error) =>
        error instanceof AccountResolutionError && error.code === fixture.expectedError,
    );
  });
}

test("resolves every generated static account", () => {
  for (const fixture of fixtures.staticAccountCases) {
    assert.equal(
      resolver.resolveStatic(fixture.instructionKey, fixture.accountName),
      fixture.expectedAddress,
      fixture.name,
    );
  }
  assert.equal(
    resolver.pdaRecipeKey("dusk:trigger_liquidation_auction", "market"),
    "dusk:market",
  );
});

test("rejects an Anchor discriminator drift in the instruction contract", () => {
  const drifted = structuredClone(contract) as unknown as {
    instructions: Array<{ discriminatorHex: string }>;
  };
  drifted.instructions[0].discriminatorHex = "00".repeat(8);
  assert.throws(
    () =>
      encodeKeeperInstructionJson(
        drifted as unknown as InstructionContract,
        "dusk:trigger_liquidation_auction",
        {},
      ),
    (error) => error instanceof InstructionEncodingError && error.code === "contract_mismatch",
  );
});

test("rejects account-resolution manifest provenance drift", () => {
  const drifted = structuredClone(manifest) as unknown as {
    programs: Array<{ programId: string }>;
  };
  drifted.programs[0].programId = "11111111111111111111111111111111";
  assert.throws(
    () =>
      new DeterministicAccountResolver(
        lock,
        contract,
        drifted as unknown as AccountResolutionManifest,
      ),
    (error) => error instanceof AccountResolutionError && error.code === "manifest_mismatch",
  );
});
