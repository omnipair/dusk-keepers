import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "node:http";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import { PublicKey } from "@solana/web3.js";
const execute = promisify(execFile);

// Local RPC evidence belongs only to temporary test roots. A source capture
// intentionally has no deployment attestation and cannot opt into live mode.
function mockDeploymentRoot() {
  const root = mkdtempSync(join(tmpdir(), "dusk-devnet-gate-"));
  const lock = JSON.parse(readFileSync("protocol.lock.json", "utf8"));
  const deployment = {
    cluster: "devnet", genesisHash: "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG",
    protocolRevision: lock.revision, sourceCommit: lock.source.gitCommit,
    programs: lock.programs.map((p: any, i: number) => ({
      name: p.name, programId: p.programId,
      programData: new PublicKey(Buffer.alloc(32, 100 + i)).toBase58(),
      lastUpgradeSlot: 500_000_000,
      binarySha256: p.binary.sha256,
      idlAccount: new PublicKey(Buffer.alloc(32, 110 + i)).toBase58(),
      idlAccountSha256: createHash("sha256").update(`mock deployed IDL ${p.name}`).digest("hex"),
    })),
  };
  mkdirSync(join(root, "protocol"));
  symlinkSync(resolve("artifacts"), join(root, "artifacts"), "dir");
  copyFileSync("protocol.lock.json", join(root, "protocol.lock.json"));
  writeFileSync(join(root, "protocol/devnet-deployment.v1.json"), JSON.stringify(deployment));
  return { root, lock, deployment };
}

for (const condition of ["normal live", "wrong network", "wrong deployment", "excess sends"] as const) {
  test(`native devnet gate rejects ${condition} before signing or submission`, async () => {
    const { root } = mockDeploymentRoot();
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
        DUSK_PROTOCOL_LOCK: join(root, "protocol.lock.json"),
        PATH: process.env.PATH, KEEPER_MODE: "live", KEEPER_PROFILE: "lending",
        KEEPER_MAX_SENDS_PER_PASS: condition === "excess sends" ? "2" : "1",
        SOLANA_RPC_HTTP_URL: `http://127.0.0.1:${(server.address() as any).port}`,
      };
      if (condition !== "normal live") env.KEEPER_ALLOW_CAPTURED_DEVNET = "1";
      const expected = { "normal live": /NotFrozen/, "wrong network": /refuses this network/, "wrong deployment": /account identity changed/, "excess sends": /MAX_SENDS_PER_PASS=1/ }[condition];
      await assert.rejects(execute("target/debug/dusk-keeper", ["--check"], { env, timeout: 20000 }), (error: any) => expected.test(error.stderr));
      assert.ok(!requests.includes("sendTransaction"));
      if (["normal live", "excess sends"].includes(condition)) assert.equal(requests.length, 0);
    } finally {
      await new Promise<void>(resolve => server.close(() => resolve()));
      rmSync(root, { recursive: true, force: true });
    }
  });
}

test("verified devnet live startup accepts all markets without a market selector", async () => {
  const { root, deployment, lock } = mockDeploymentRoot();
  const accounts = new Map<string, object>();
  const loader = "BPFLoaderUpgradeab1e11111111111111111111111";
  const account = (data: Buffer, owner: string, executable = false) => ({
    data: [data.toString("base64"), "base64"], owner, executable, lamports: 1_000_000,
  });
  for (const program of deployment.programs) {
    const executable = Buffer.alloc(36);
    executable.writeUInt32LE(2);
    new PublicKey(program.programData).toBuffer().copy(executable, 4);
    const header = Buffer.alloc(45);
    header.writeUInt32LE(3);
    header.writeBigUInt64LE(BigInt(program.lastUpgradeSlot), 4);
    const binary = readFileSync(lock.programs.find((p: any) => p.name === program.name).binary.path);
    const idl = Buffer.from(`mock deployed IDL ${program.name}`);
    program.idlAccountSha256 = createHash("sha256").update(idl).digest("hex");
    accounts.set(program.programId, account(executable, loader, true));
    accounts.set(program.programData, account(Buffer.concat([header, binary]), loader));
    accounts.set(program.idlAccount, account(idl, program.programId));
  }
  const requests: string[] = [];
  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString());
    requests.push(body.method);
    const context = { slot: 600_000_000 };
    const result = body.method === "getGenesisHash" ? deployment.genesisHash
      : body.method === "getMultipleAccounts" ? { context, value: body.params[0].map((address: string) => accounts.get(address) ?? null) }
      : { context, value: accounts.get(body.params[0]) ?? null };
    res.end(JSON.stringify({ jsonrpc: "2.0", id: body.id, result }));
  });
  try {
    await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
    const { stdout } = await execute("target/debug/dusk-keeper", ["--check"], {
      env: {
        PATH: process.env.PATH, KEEPER_MODE: "live", KEEPER_PROFILE: "lending",
        KEEPER_ALLOW_CAPTURED_DEVNET: "1", KEEPER_MAX_SENDS_PER_PASS: "1",
        DUSK_PROTOCOL_LOCK: join(root, "protocol.lock.json"),
        SOLANA_RPC_HTTP_URL: `http://127.0.0.1:${(server.address() as any).port}`,
      }, timeout: 20000,
    });
    assert.match(stdout, /verified explicit devnet test exception/);
    assert.match(stdout, /profile=lending mode=live/);
    assert.equal(requests.filter(method => method === "getMultipleAccounts").length, deployment.programs.length);
    assert.equal(requests.filter(method => method === "getAccountInfo").length, deployment.programs.length);
    assert.ok(!requests.includes("sendTransaction"));
  } finally {
    if (server.listening) await new Promise<void>(resolve => server.close(() => resolve()));
    rmSync(root, { recursive: true, force: true });
  }
});
