import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "node:http";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
const execute = promisify(execFile);

for (const condition of ["normal live", "wrong network", "wrong deployment", "missing market", "excess sends"] as const) {
  test(`native devnet gate rejects ${condition} before signing or submission`, async () => {
    const requests: string[] = [];
    const server = createServer(async (req, res) => {
      const chunks: Buffer[] = [];
      for await (const chunk of req) chunks.push(chunk);
      const body = JSON.parse(Buffer.concat(chunks).toString());
      requests.push(body.method);
      res.end(JSON.stringify({ jsonrpc: "2.0", id: body.id, result: body.method === "getGenesisHash"
        ? condition === "wrong network" ? "mainnet" : "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG"
        : { context: { slot: 1 }, value: [null, null] } }));
    });
    await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
    try {
      const env: NodeJS.ProcessEnv = {
        PATH: process.env.PATH, KEEPER_MODE: "live", KEEPER_PROFILE: "lending",
        KEEPER_MAX_SENDS_PER_PASS: condition === "excess sends" ? "2" : "1",
        SOLANA_RPC_HTTP_URL: `http://127.0.0.1:${(server.address() as any).port}`,
      };
      if (condition !== "normal live") env.KEEPER_ALLOW_CAPTURED_DEVNET = "1";
      if (condition !== "missing market") env.KEEPER_DEVNET_MARKET = "EdqUE8D7JuTrdSAy4kpynjvcaKugguuyjHzC6vFvw3HD";
      const expected = { "normal live": /NotFrozen/, "wrong network": /refuses this network/, "wrong deployment": /account identity changed/, "missing market": /requires one KEEPER_DEVNET_MARKET/, "excess sends": /MAX_SENDS_PER_PASS=1/ }[condition];
      await assert.rejects(execute("target/debug/dusk-keeper", ["--check"], { env, timeout: 20000 }), (error: any) => expected.test(error.stderr));
      assert.ok(!requests.includes("sendTransaction"));
      if (["normal live", "missing market", "excess sends"].includes(condition)) assert.equal(requests.length, 0);
    } finally {
      await new Promise<void>(resolve => server.close(() => resolve()));
    }
  });
}
