import assert from "node:assert/strict";
import test from "node:test";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const execute = promisify(execFile);
const cleanEnv = { PATH: process.env.PATH };

test("read-only deployment comparison checks genesis without enabling captured live signing", async t => {
  const calls: string[] = [];
  const rpc = createServer(async (request, response) => {
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(chunk);
    const call = JSON.parse(Buffer.concat(chunks).toString());
    calls.push(call.method);
    response.end(JSON.stringify({ jsonrpc: "2.0", id: call.id, result: "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG" }));
  });
  await new Promise<void>(resolve => rpc.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise<void>(resolve => rpc.close(() => resolve())));
  const env = { ...cleanEnv, SOLANA_RPC_HTTP_URL: `http://127.0.0.1:${(rpc.address() as any).port}` };
  await assert.rejects(execute("target/debug/dusk-keeper", ["--check-mainnet-deployment"], { env }), /refuses this RPC genesis/);
  assert.deepEqual(calls, ["getGenesisHash"]);
  await assert.rejects(execute("target/debug/dusk-keeper", ["--check-execution"], {
    env: { ...env, KEEPER_MODE: "live", KEEPER_PROFILE: "lifecycle" },
  }), /NotFrozen/);
  assert.deepEqual(calls, ["getGenesisHash"], "captured execution stops before signing or RPC");
});

test("mainnet rejects missing policies and spending settings before reaching signer or RPC", async t => {
  const directory = mkdtempSync(join(tmpdir(), "keeper-policy-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  // A temporary fixture exercises the gates after the frozen-status check; it
  // neither modifies nor promotes the repository's captured release.
  const lock = JSON.parse(readFileSync("protocol.lock.json", "utf8"));
  lock.status = "frozen";
  const path = join(directory, "protocol.lock.json");
  writeFileSync(path, JSON.stringify(lock));
  const env = { ...cleanEnv, DUSK_PROTOCOL_LOCK: path, KEEPER_MODE: "live" };
  await assert.rejects(execute("target/debug/dusk-keeper", ["--check-execution"], {
    env: { ...env, KEEPER_PROFILE: "lifecycle" },
  }), /KEEPER_RISK_POLICY is required/);
  await assert.rejects(execute("target/debug/dusk-keeper", ["--check-execution"], {
    env: { ...env, KEEPER_PROFILE: "auction-arbitrage" },
  }), /KEEPER_AUCTION_MAX_SOLD must be configured/);
  const policy = join(directory, "bad-policy.json");
  writeFileSync(policy, JSON.stringify({ schemaVersion: 1, profiles: { lifecycle: {
    fees: { perTransaction: "200000", perDay: "1" }, minimumSolReserve: "50000000", tokens: {},
  } } }));
  await assert.rejects(execute("target/debug/dusk-keeper", ["--check-execution"], {
    env: { ...env, KEEPER_PROFILE: "lifecycle", KEEPER_RISK_POLICY: policy },
  }), /daily limits must cover a transaction/);
});
