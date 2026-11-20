import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";

for (const [name, provider, mode, extra, message] of [
  ["local keys", "local", "shadow", [], "KMS canary requires only"],
  ["live mode", "aws-kms", "live", [], "run the KMS canary while the service is in shadow mode"],
  ["arbitrary CLI payloads", "aws-kms", "shadow", ["transaction-bytes"], "KMS canary requires only"],
] as const) {
  test(`KMS readiness rejects ${name} before loading a signer`, () => {
    const child = spawnSync("target/debug/dusk-keeper", ["--check-kms-signer", ...extra], {
      encoding: "utf8",
      timeout: 5000,
      env: { ...process.env, SIGNER_PROVIDER: provider, KEEPER_MODE: mode,
        KEEPER_PROFILE: "leverage", AWS_EC2_METADATA_DISABLED: "true", KEEPER_SIGNER_KEY_PATH: "/nonexistent/canary-must-not-read-this" },
    });
    assert.equal(child.error, undefined);
    assert.equal(child.status, 1);
    assert.equal(child.stdout, "");
    assert.ok(child.stderr.includes(message), child.stderr);
    assert.ok(!child.stderr.includes("unreadable"), "canary must reject before key access");
  });
}
