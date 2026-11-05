// Explicit, reviewable candidate capture. Never freezes a lock or deploys.
import assert from "node:assert/strict";
import { readFile, writeFile, copyFile, rm } from "node:fs/promises";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { compatibilityFingerprints } from "./compatibility-fingerprints.mjs";
const [sourceArg, sdkArg, revision] = process.argv.slice(2);
assert.ok(
  sourceArg && sdkArg && revision,
  "usage: capture-protocol.mjs <clean-dusk-checkout> <packed-sdk.tgz> <revision>",
);
const source = resolve(sourceArg),
  sdk = resolve(sdkArg);
assert.equal(
  execFileSync(
    "git",
    ["-C", source, "status", "--porcelain", "--untracked-files=no"],
    { encoding: "utf8" },
  ).trim(),
  "",
  "source must have no tracked modifications",
);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const lock = JSON.parse(await readFile("protocol.lock.json", "utf8"));
const oldRevision = lock.revision;
lock.revision = revision;
lock.status = "captured";
lock.generatedAt = new Date().toISOString();
delete lock.cluster;
// Record the capture environment instead of inheriting a previous revision's tools.
const version = (command, args = ["--version"]) => {
  try {
    return execFileSync(command, args, { encoding: "utf8", cwd: source, stdio: ["ignore", "pipe", "ignore"] }).trim().split("\n")[0];
  } catch {
    return null;
  }
};
lock.toolchain = {
  anchor: version("anchor"), rust: version("rustc"), solana: version("solana"),
  surfpool: version("surfpool"), node: process.version,
};
lock.source = {
  repository: "https://github.com/omnipair/dusk",
  branch: execFileSync("git", ["-C", source, "branch", "--show-current"], {
    encoding: "utf8",
  }).trim() || "(detached)",
  gitCommit: execFileSync("git", ["-C", source, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim(),
  worktreeFingerprintSha256: execFileSync(
    process.execPath,
    ["scripts/compute-worktree-fingerprint.mjs", source],
    { encoding: "utf8" },
  ).split(" ")[0],
};
for (const program of lock.programs) {
  for (const [kind, input, output] of [
    [
      "idl",
      `target/idl/${program.name}.json`,
      `artifacts/${program.name}.json`,
    ],
    [
      "binary",
      `target/deploy/${program.name}.so`,
      `artifacts/${program.name}.so`,
    ],
  ]) {
    await copyFile(`${source}/${input}`, output);
    program[kind] = { path: output, sha256: sha(await readFile(output)) };
  }
  program.programId = JSON.parse(
    await readFile(program.idl.path, "utf8"),
  ).address;
}
await copyFile(sdk, lock.sdk.path);
lock.sdk.sha256 = sha(await readFile(lock.sdk.path));
lock.compatibility = compatibilityFingerprints(
  await Promise.all(
    lock.programs.map(async (p) =>
      JSON.parse(await readFile(p.idl.path, "utf8")),
    ),
  ),
);
const json = (v) => JSON.stringify(v, null, 2) + "\n";
await writeFile("protocol.lock.json", json(lock));
for (const file of ["scheduler-cases", "execution-lifecycle-cases"]) {
  const path = `fixtures/conformance/v1/${file}.json`;
  await writeFile(
    path,
    (await readFile(path, "utf8")).replaceAll(
      JSON.stringify(oldRevision),
      JSON.stringify(revision),
    ),
  );
}
const path = "fixtures/conformance/v1/health-provenance.json",
  health = JSON.parse(await readFile(path, "utf8"));
health.protocol.revision = revision;
health.protocol.lockStatus = lock.status;
health.protocol.sourceWorktreeFingerprintSha256 =
  lock.source.worktreeFingerprintSha256;
// Preserve schema field names; this capture does not invent release attestations.
health.protocol.lockSha256 = sha(Buffer.from(json(lock)));
await writeFile(path, json(health));
for (const script of [
  "generate-instruction-contract",
  "generate-adapter-codecs",
  "generate-account-layout",
])
  execFileSync(process.execPath, [`scripts/${script}.mjs`, "--write"], {
    stdio: "inherit",
  });
// A generic source capture does not inherit a previous deployment observation.
// capture-devnet.mjs writes fresh evidence after this command succeeds.
await rm("protocol/devnet-deployment.v1.json", { force: true });
console.log(
  "Candidate captured. Refresh health provenance and bank fixtures; run the repository checks before committing.",
);
