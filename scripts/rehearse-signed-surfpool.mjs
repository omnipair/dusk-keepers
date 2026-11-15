// Execute public captured plans on local Surfpool with disposable in-memory signers.
// This receipt covers captured liquidation/hLP cases, not the full live-release matrix.
import assert from "node:assert/strict";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { pathToFileURL, fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("..", import.meta.url)).replace(/\/$/, "");
process.chdir(root);
const require = createRequire(root + "/package.json");
const { Keypair, PublicKey, VersionedTransaction } = require("@solana/web3.js");
const {
  getAssociatedTokenAddressSync,
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
} = require("@solana/spl-token");
const { nativePlan } = await import(
  pathToFileURL(root + "/typescript/tests/helpers/native-planner.ts")
);
const { payment } = await import(
  pathToFileURL(root + "/typescript/tests/helpers/liquidation-bank.ts")
);
const { decodeLiquidationQuote } = await import(
  pathToFileURL(root + "/typescript/packages/dusk-adapter/src/liquidation.ts")
);
const endpoint = process.env.SURFPOOL_REHEARSAL_RPC ?? "http://127.0.0.1:18899";
const target = new URL(endpoint);
if (
  target.protocol !== "http:" ||
  !["127.0.0.1", "localhost", "[::1]"].includes(target.hostname) ||
  target.username ||
  target.password ||
  target.search ||
  target.pathname !== "/"
)
  throw new Error("Signed rehearsal requires an isolated local HTTP RPC");
let id = 0;
async function rpc(method, params = []) {
  const r = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }),
    signal: AbortSignal.timeout(20000),
  });
  const j = await r.json();
  if (j.error) throw Error(method + ": " + JSON.stringify(j.error));
  return j.result;
}
const genesis = await rpc("getGenesisHash");
assert.notEqual(
  genesis,
  "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d",
  "local genesis",
);
const lock = JSON.parse(readFileSync(root + "/protocol.lock.json"));
for (const p of lock.programs) {
  const bytes = readFileSync(root + "/" + p.binary.path);
  assert.equal(
    createHash("sha256").update(bytes).digest("hex"),
    p.binary.sha256,
  );
  await rpc("surfnet_writeProgram", [p.programId, bytes.toString("hex"), 0]);
}
async function setAccount(address, a) {
  await rpc("surfnet_setAccount", [
    address,
    {
      owner: a.owner,
      executable: a.executable,
      lamports: Number(a.lamports ?? 10000000),
      data: Buffer.from(a.data).toString("hex"),
    },
  ]);
}
const rows = [];
for (const directory of ["fixtures/litesvm", "fixtures/hlp-litesvm"])
  for (const file of readdirSync(root + "/" + directory)
    .filter((f) => f.endsWith(".json"))
    .sort()) {
    const original = JSON.parse(
      readFileSync(root + "/" + directory + "/" + file),
    );
    assert.equal(
      original.sourceCommit,
      lock.source.gitCommit,
      "fixture source matches the pinned executable",
    );
    const signer = Keypair.generate(),
      buyer = signer.publicKey.toBase58(),
      oldBuyer = new PublicKey(original.buyer).toBuffer();
    const names = new Map([[original.buyer, buyer]]),
      owned = new Set();
    for (const [address, a] of Object.entries(original.bank)) {
      if (
        ![
          TOKEN_PROGRAM_ID.toBase58(),
          TOKEN_2022_PROGRAM_ID.toBase58(),
        ].includes(a.owner) ||
        a.data.length < 165 ||
        !Buffer.from(a.data).subarray(32, 64).equals(oldBuyer)
      )
        continue;
      const mint = new PublicKey(Buffer.from(a.data).subarray(0, 32));
      const next = getAssociatedTokenAddressSync(
        mint,
        signer.publicKey,
        false,
        new PublicKey(a.owner),
      ).toBase58();
      assert.ok(
        !owned.has(next),
        "fixture has one signer token account per mint",
      );
      owned.add(next);
      names.set(address, next);
    }
    const rewrite = (v) =>
      typeof v === "string"
        ? (names.get(v) ?? v)
        : Array.isArray(v)
          ? v.map(rewrite)
          : v && typeof v === "object"
            ? Object.fromEntries(
                Object.entries(v).map(([k, x]) => [
                  names.get(k) ?? k,
                  rewrite(x),
                ]),
              )
            : v;
    const f = rewrite(original);
    for (const accounts of [f.bank, f.snapshot.accounts])
      for (const [address, a] of Object.entries(accounts)) {
        if (!owned.has(address)) continue;
        const bytes = Buffer.from(a.data);
        signer.publicKey.toBuffer().copy(bytes, 32);
        a.data = [...bytes];
      }
    const clock = Buffer.from(
      f.bank.SysvarC1ock11111111111111111111111111111111.data,
    );
    const slot = Number(clock.readBigUInt64LE());
    const current = await rpc("getSlot", [{ commitment: "processed" }]);
    await rpc("surfnet_timeTravel", [
      { absoluteSlot: Math.max(slot, current + 100) },
    ]);
    for (const [address, a] of Object.entries(f.bank))
      if (!a.executable) await setAccount(address, a);
    await setAccount(buyer, {
      owner: "11111111111111111111111111111111",
      data: [],
      lamports: 1000000000,
      executable: false,
    });
    const quote =
      f.action === "flash"
        ? decodeLiquidationQuote(f.action, Buffer.from(f.quoteBase64, "base64"))
        : null;
    const request =
      f.action === "flash"
        ? { ...f, internal: false, route: [payment(f, quote)] }
        : f;
    let plan = nativePlan(request);
    const addresses = [
      ...new Set(
        plan.instructions.flatMap((ix) =>
          ix.accounts
            .filter((a) => !a.is_signer)
            .map((a) => new PublicKey(a.pubkey).toBase58()),
        ),
      ),
    ];
    const table = new PublicKey(
        createHash("sha256")
          .update(file + buyer)
          .digest(),
      ).toBase58(),
      data = Buffer.alloc(56 + addresses.length * 32);
    data.writeUInt32LE(1);
    data.writeBigUInt64LE(0xffffffffffffffffn, 4);
    addresses.forEach((key, i) =>
      new PublicKey(key).toBuffer().copy(data, 56 + i * 32),
    );
    await setAccount(table, {
      owner: "AddressLookupTab1e1111111111111111111111111",
      executable: false,
      data,
      lamports: 10000000,
    });
    plan = nativePlan({
      ...request,
      lookupTable: table,
      lookupAddresses: addresses,
    });
    const tx = VersionedTransaction.deserialize(
      Buffer.concat([
        Buffer.from([1]),
        Buffer.alloc(64),
        Buffer.from(plan.messageBase64, "base64"),
      ]),
    );
    assert.equal(tx.message.header.numRequiredSignatures, 1);
    assert.ok(tx.message.staticAccountKeys[0].equals(signer.publicKey));
    tx.message.recentBlockhash = (
      await rpc("getLatestBlockhash", [{ commitment: "processed" }])
    ).value.blockhash;
    tx.sign([signer]);
    const wire = Buffer.from(tx.serialize()).toString("base64");
    const forged = Buffer.from(tx.serialize());
    forged[1] ^= 1;
    const forgedSimulation = await rpc("simulateTransaction", [
      forged.toString("base64"),
      { encoding: "base64", sigVerify: true, commitment: "processed" },
    ]);
    assert.equal(
      forgedSimulation.value.err,
      "SignatureFailure",
      "validator must reject a forged signature",
    );
    const simulated = await rpc("simulateTransaction", [
      wire,
      { encoding: "base64", sigVerify: true, commitment: "processed" },
    ]);
    assert.equal(
      simulated.value.err,
      null,
      file +
        ": " +
        JSON.stringify(simulated.value.err) +
        " " +
        (simulated.value.logs ?? []).slice(-8).join("\n"),
    );
    const signature = await rpc("sendTransaction", [
      wire,
      {
        encoding: "base64",
        skipPreflight: false,
        preflightCommitment: "processed",
        maxRetries: 0,
      },
    ]);
    let status;
    for (let i = 0; i < 20; i++) {
      status = (await rpc("getSignatureStatuses", [[signature]])).value[0];
      if (status) break;
      await new Promise((r) => setTimeout(r, 100));
    }
    assert.ok(status, file + " landed");
    assert.equal(status.err, null);
    if (plan.named.session) {
      const session = new PublicKey(plan.named.session).toBase58(),
        value = (
          await rpc("getAccountInfo", [
            session,
            { encoding: "base64", commitment: "processed" },
          ])
        ).value;
      assert.ok(
        value === null || value.lamports === 0,
        "flash session closes after committed execution",
      );
    }
    rows.push({
      fixture: directory + "/" + file,
      action: f.action,
      kind: f.kind,
      sourceCommit: f.sourceCommit,
      slot: status.slot,
      signature,
      signer: buyer,
      signatureVerified: true,
      forgedSignatureRejected: true,
      status: "passed",
    });
    console.log(file + " signed execution passed");
  }
writeFileSync(
  process.env.SURFPOOL_REHEARSAL_PROOF ??
    "/private/tmp/dusk-signed-keeper-proof.json",
  JSON.stringify(
    {
      schemaVersion: 1,
      kind: "signed-offline-native-plan-execution",
      genesis,
      protocolRevision: lock.revision,
      completeLiveReleaseMatrix: false,
      cases: rows,
    },
    null,
    2,
  ) + "\n",
);
console.log("Signed local cases: " + rows.length);
