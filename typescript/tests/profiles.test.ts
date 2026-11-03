import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";

const profiles = JSON.parse(readFileSync("deploy/railway/profiles.json", "utf8"));
const runtimes = [
  ["target/debug/dusk-keeper", ["--check"]],
  [process.execPath, ["--experimental-strip-types", "typescript/apps/keeper/src/index.ts", "--check"]],
] as const;

for (const [command, args] of runtimes) {
  test(`${command} accepts deployed profiles and rejects removed or unknown profiles`, () => {
    for (const profile of [...profiles.services.map((service: { profile: string }) => service.profile), "conformance", "lending-trigger", "lending-bidder", "lending-settler", "typo"]) {
      const result = spawnSync(command, [...args], {
        encoding: "utf8",
        env: { ...process.env, KEEPER_MODE: "shadow", KEEPER_PROFILE: profile, KEEPER_LENDING_MODE: "both" },
      });
      assert.ifError(result.error);
      const removed = ["lending-trigger", "lending-bidder", "lending-settler", "typo"].includes(profile);
      assert.equal(result.status === 0, !removed, result.stdout + result.stderr);
      if (removed) assert.match(result.stderr, /unsupported KEEPER_PROFILE/);
    }
  });
}

for (const [command, args] of runtimes) {
  test(`${command} validates lending execution modes`, () => {
    for (const mode of ["ordinary", "emergency", "both", "typo"]) {
      const result = spawnSync(command, [...args], { encoding: "utf8", env: { ...process.env, KEEPER_MODE: "shadow", KEEPER_PROFILE: "lending", KEEPER_LENDING_MODE: mode } });
      assert.ifError(result.error);
      assert.equal(result.status === 0, mode !== "typo", result.stdout + result.stderr);
      if (mode === "typo") assert.match(result.stderr, /KEEPER_LENDING_MODE/);
    }
  });
}
