import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

const root = new URL("../", import.meta.url);

async function readJson(path) {
  const raw = await readFile(new URL(path, root), "utf8");
  return JSON.parse(raw);
}

const schemaPaths = [
  "protocol/schemas/protocol-lock.schema.json",
  "protocol/schemas/candidate-intent.schema.json",
  "protocol/schemas/execution-outcome.schema.json",
  "protocol/schemas/scheduler-fixture.schema.json",
  "protocol/schemas/expected-race.schema.json",
  "protocol/schemas/execution-event.schema.json",
  "protocol/schemas/execution-attempt.schema.json",
  "protocol/schemas/lifecycle-fixture.schema.json",
  "protocol/schemas/health-provenance.schema.json",
  "protocol/schemas/deployment-profiles.schema.json",
];

for (const path of schemaPaths) {
  const schema = await readJson(path);
  assert.equal(schema.$schema, "https://json-schema.org/draft/2020-12/schema");
  assert.equal(typeof schema.$id, "string");
}

const lock = await readJson("protocol.lock.json");
assert.equal(lock.schemaVersion, 1);
assert.match(lock.source.gitCommit, /^[0-9a-f]{40}$/);
assert.ok(["draft", "captured", "frozen"].includes(lock.status));
assert.ok(lock.programs.some((program) => program.name === "dusk"));
assert.ok(lock.programs.some((program) => program.name === "leverage_delegate"));

async function verifyArtifact(artifact, label) {
  assert.match(artifact.sha256, /^[0-9a-f]{64}$/, `${label}: missing SHA-256`);
  const bytes = await readFile(new URL(artifact.path, root));
  const actual = createHash("sha256").update(bytes).digest("hex");
  assert.equal(actual, artifact.sha256, `${label}: artifact hash mismatch`);
}

if (lock.status !== "draft") {
  assert.match(lock.source.worktreeFingerprintSha256, /^[0-9a-f]{64}$/);
  for (const program of lock.programs) {
    assert.ok(program.programId, `${program.name}: missing program ID`);
    await verifyArtifact(program.binary, `${program.name}.binary`);
    await verifyArtifact(program.idl, `${program.name}.idl`);
  }
  await verifyArtifact(lock.sdk, "sdk");
  for (const [name, hash] of Object.entries(lock.compatibility)) {
    assert.match(hash, /^[0-9a-f]{64}$/, `compatibility.${name}: missing fingerprint`);
  }
}

const bundle = await readJson("fixtures/conformance/v1/scheduler-cases.json");
assert.equal(bundle.schemaVersion, 1);
assert.ok(bundle.cases.length > 0);

for (const fixtureCase of bundle.cases) {
  assert.ok(fixtureCase.name.length > 0);
  assert.ok(Array.isArray(fixtureCase.expectedOrder));
  const ids = new Set();
  for (const candidate of fixtureCase.candidates) {
    assert.ok(!ids.has(candidate.candidateId), `${fixtureCase.name}: duplicate candidateId`);
    ids.add(candidate.candidateId);
    assert.equal(candidate.expectedStateHash.length, 64);
    assert.equal(candidate.protocolRevision, lock.revision);
  }
}

const lifecycleBundle = await readJson("fixtures/conformance/v1/execution-lifecycle-cases.json");
assert.equal(lifecycleBundle.schemaVersion, 1);
assert.ok(lifecycleBundle.cases.length > 0);
assert.equal(
  new Set(lifecycleBundle.cases.map((fixtureCase) => fixtureCase.workKey)).size,
  lifecycleBundle.cases.length,
  "lifecycle work keys must be unique",
);
for (const fixtureCase of lifecycleBundle.cases) {
  assert.match(fixtureCase.workKey, /^[0-9a-f]{64}$/);
  assert.equal(fixtureCase.candidate.protocolRevision, lock.revision);
  assert.ok(fixtureCase.events.length > 0);
}
assert.ok(
  lifecycleBundle.cases.some((fixtureCase) =>
    fixtureCase.events.some(
      (event) => event.type === "blockhash_expired" && event.timing === "before_submit",
    ),
  ),
  "missing before-submit expiry fixture",
);
assert.ok(
  lifecycleBundle.cases.some((fixtureCase) =>
    fixtureCase.events.some((event) => event.type === "submitted_not_landed_finalized"),
  ),
  "missing finalized-not-landed reconciliation fixture",
);
assert.ok(
  lifecycleBundle.cases.some((fixtureCase) =>
    fixtureCase.events.some((event) => event.type === "submitted_landed"),
  ),
  "missing late-landing reconciliation fixture",
);

const health = await readJson("fixtures/conformance/v1/health-provenance.json");
assert.equal(health.schemaVersion, 1);
assert.equal(health.protocol.revision, lock.revision);
assert.equal(health.protocol.lockStatus, lock.status);
assert.equal(health.protocol.sourceWorktreeFingerprintSha256, lock.source.worktreeFingerprintSha256);
const lockBytes = await readFile(new URL("protocol.lock.json", root));
assert.equal(
  health.protocol.lockSha256,
  createHash("sha256").update(lockBytes).digest("hex"),
  "health fixture protocol lock hash is stale",
);

const profiles = await readJson("deploy/railway/profiles.json");
assert.equal(profiles.schemaVersion, 1);
assert.ok(profiles.services.length > 0);
assert.equal(new Set(profiles.services.map((service) => service.name)).size, profiles.services.length);
assert.equal(
  new Set(profiles.services.map((service) => service.walletRole)).size,
  profiles.services.length,
  "each service must have a distinct wallet role",
);
for (const requiredProfile of [
  "lending-trigger",
  "lending-bidder",
  "lending-settler",
  "sentinel",
]) {
  assert.ok(
    profiles.services.some((service) => service.profile === requiredProfile),
    `missing ${requiredProfile} Railway profile`,
  );
}
const sentinel = profiles.services.find((service) => service.profile === "sentinel");
assert.equal(sentinel.signingRequired, false);
assert.equal(sentinel.walletRole, "none");
for (const service of profiles.services.filter((service) => service.profile !== "sentinel")) {
  assert.equal(service.signingRequired, true, `${service.profile}: signing policy must be explicit`);
  assert.notEqual(service.walletRole, "none", `${service.profile}: missing isolated wallet role`);
}

const migration = await readFile(
  new URL("deploy/postgres/migrations/0001_keeper_attempts.sql", root),
  "utf8",
);
assert.match(migration, /work_key CHAR\(64\) PRIMARY KEY/);
assert.match(migration, /keeper_logical_work_unique UNIQUE/);
assert.match(migration, /CREATE TABLE keeper_signing_generations/);
assert.match(migration, /submitted_unknown/);

if (process.env.REQUIRE_FROZEN_PROTOCOL === "1") {
  assert.equal(lock.status, "frozen", "live deployment requires a frozen protocol lock");
  assert.ok(lock.source.worktreeFingerprintSha256, "missing worktree fingerprint");
  for (const program of lock.programs) {
    assert.ok(program.programId, `${program.name}: missing program id`);
    assert.ok(program.binary.sha256, `${program.name}: missing binary hash`);
    assert.ok(program.idl.sha256, `${program.name}: missing IDL hash`);
  }
}

console.log(
  `validated ${schemaPaths.length} schemas, ${bundle.cases.length} scheduler cases, ${lifecycleBundle.cases.length} lifecycle cases, ${profiles.services.length} service profiles, and ${lock.revision} (${lock.status})`,
);
