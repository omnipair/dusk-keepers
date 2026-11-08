import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer, type Server } from "node:http";
import { setTimeout as delay } from "node:timers/promises";
import test, { type TestContext } from "node:test";

async function listen(server: Server): Promise<number> {
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
  return (server.address() as { port: number }).port;
}

async function close(server: Server): Promise<void> {
  server.closeAllConnections();
  await new Promise<void>(resolve => server.close(() => resolve()));
}

async function startKeeper(
  t: TestContext,
  environment: NodeJS.ProcessEnv = {},
  handleRpc: (request: any) => Promise<object> = async request => ({
    result: request.method === "getSlot" ? 123 : [],
  }),
) {
  // No inherited credentials, deployment overrides, or real network endpoints.
  const rpc = createServer(async (request, response) => {
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString());
    response.end(JSON.stringify({ jsonrpc: "2.0", id: body.id, ...await handleRpc(body) }));
  });
  const rpcPort = await listen(rpc);
  t.after(() => close(rpc));
  const reservation = createServer();
  const port = await listen(reservation);
  await close(reservation);
  const child = spawn("target/debug/dusk-keeper", [], {
    env: {
      PATH: process.env.PATH,
      KEEPER_MODE: "shadow",
      KEEPER_PROFILE: "lending",
      KEEPER_SHADOW_PUBLIC_KEY: "11111111111111111111111111111111",
      SOLANA_RPC_HTTP_URL: `http://127.0.0.1:${rpcPort}`,
      DISCOVERY_INTERVAL_MS: "60000",
      PORT: String(port),
      ...environment,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", chunk => { output += chunk; });
  child.stderr.on("data", chunk => { output += chunk; });
  const stopped = new Promise<void>(resolve => child.once("close", () => resolve()));
  t.after(async () => { child.kill(); await stopped; });
  return async (matches: (body: any) => boolean, path = "/readyz") => {
    const deadline = Date.now() + 10000;
    let last: any;
    while (Date.now() < deadline) {
      assert.equal(child.exitCode, null, output);
      try {
        const response = await fetch(`http://127.0.0.1:${port}${path}`, {
          signal: AbortSignal.timeout(1000),
        });
        last = await response.json();
        if (matches(last)) return { status: response.status, body: last };
      } catch { /* Wait for the listener to bind. */ }
      await delay(25);
    }
    assert.fail(`health endpoint did not reach expected state: ${JSON.stringify(last)}\n${output}`);
  };
}

test("configured keeper stays starting until discovery and execution checks finish", async t => {
  let releaseDiscovery!: () => void;
  let releaseExecution!: () => void;
  const discovery = new Promise<void>(resolve => { releaseDiscovery = resolve; });
  const execution = new Promise<void>(resolve => { releaseExecution = resolve; });
  let accountReads = 0;
  const health = await startKeeper(t, {}, async request => {
    if (request.method === "getSlot") await discovery;
    // Discovery reads three account types, fee monitoring reads markets,
    // and then the lending worker starts its own fresh execution scan.
    if (request.method === "getProgramAccounts" && ++accountReads === 5) await execution;
    return { result: request.method === "getSlot" ? 123 : [] };
  });
  try {
    const initial = await health(body => body.passes === 0);
    assert.equal(initial.status, 503);
    assert.equal(initial.body.status, "starting");
    assert.equal(initial.body.executionConfigured, true);
    assert.equal(initial.body.firstPassComplete, false);
    assert.equal(initial.body.reason, undefined);
    assert.equal((await health(() => true, "/healthz")).status, 200);

    releaseDiscovery();
    const scanning = await health(body => body.executionPasses === 1);
    assert.equal(scanning.status, 503);
    assert.equal(scanning.body.passes, 1);
    assert.equal(scanning.body.status, "starting");
    assert.equal(scanning.body.firstPassComplete, false);

    releaseExecution();
    const ready = await health(body => body.firstPassComplete);
    assert.equal(ready.status, 200);
    assert.equal(ready.body.status, "ready");
    assert.equal(ready.body.sent, 0);
  } finally {
    releaseDiscovery();
    releaseExecution();
  }
});

test("shadow keeper without an identity reports the missing setting", async t => {
  const health = await startKeeper(t, { KEEPER_SHADOW_PUBLIC_KEY: "" });
  const result = await health(body => body.firstPassComplete);
  assert.equal(result.status, 503);
  assert.equal(result.body.status, "degraded");
  assert.equal(result.body.executionConfigured, false);
  assert.match(result.body.reason, /valid KEEPER_SHADOW_PUBLIC_KEY/);
});

test("sentinel becomes ready without an execution identity", async t => {
  const health = await startKeeper(t, { KEEPER_PROFILE: "sentinel", KEEPER_SHADOW_PUBLIC_KEY: "" });
  const result = await health(body => body.firstPassComplete);
  assert.equal(result.status, 200);
  assert.equal(result.body.status, "ready");
  assert.equal(result.body.executionConfigured, false);
  assert.equal(result.body.executionPasses, 0);
});

test("missing RPC configuration is degraded rather than permanently starting", async t => {
  const health = await startKeeper(t, { SOLANA_RPC_HTTP_URL: "" });
  const result = await health(() => true);
  assert.equal(result.status, 503);
  assert.equal(result.body.status, "degraded");
  assert.match(result.body.reason, /SOLANA_RPC_HTTP_URL is not configured/);
});

test("RPC discovery failure still fails readiness while liveness remains healthy", async t => {
  const health = await startKeeper(t, {}, async request => request.method === "getSlot"
    ? { error: { code: -32000, message: "fixture discovery unavailable" } }
    : { result: [] });
  const result = await health(body => body.firstPassComplete);
  assert.equal(result.status, 503);
  assert.equal(result.body.status, "degraded");
  assert.match(result.body.reason, /fixture discovery unavailable/);
  assert.equal((await health(() => true, "/healthz")).status, 200);
});
