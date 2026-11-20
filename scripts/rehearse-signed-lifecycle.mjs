// Execute the native lifecycle worker plans on an isolated local validator with real disposable signatures.
// Notice digests are created with the successor chosen before announcement.
import assert from "node:assert/strict";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { pathToFileURL, fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("..", import.meta.url)).replace(/\/$/, "");
process.chdir(root);
const require = createRequire(root + "/package.json");
const {
  Keypair,
  PublicKey,
  VersionedTransaction,
  TransactionInstruction,
  TransactionMessage,
  AddressLookupTableAccount,
} = require("@solana/web3.js");
const {
  getAssociatedTokenAddressSync,
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
} = require("@solana/spl-token");
const { nativePlan } = await import(
  pathToFileURL(root + "/typescript/tests/helpers/native-planner.ts")
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
const { fixture, run, installLookupTable, codec, program, pda, u64 } =
  await import(
    pathToFileURL(root + "/typescript/tests/helpers/lifecycle-bank.ts")
  );
const rows = [];
for (const change of ["manager", "recipient", "handoff"])
  for (const withTable of [false, true]) {
    const signer = Keypair.generate(),
      f = fixture(change, signer.publicKey);
    f.put(signer.publicKey, PublicKey.default, Buffer.alloc(0));
    f.advance(BigInt(f.notice.execute_after_slot.toString()));
    const table = withTable ? installLookupTable(f) : undefined;
    const native = await run(f, signer.publicKey, table);
    assert.equal(native.simulations.length, 1, native.output);
    assert.equal(native.simulations[0].error, null, native.output);
    const tx = native.simulations[0].transaction;
    assert.equal(tx.message.header.numRequiredSignatures, 1);
    assert.ok(tx.message.staticAccountKeys[0].equals(signer.publicKey));
    const current = await rpc("getSlot", [{ commitment: "processed" }]);
    await rpc("surfnet_timeTravel", [
      { absoluteSlot: Math.max(Number(f.svm.getClock().slot), current + 100) },
    ]);
    for (const address of f.known) {
      const a = f.svm.getAccount(new PublicKey(address));
      if (a && !a.executable)
        await setAccount(address, {
          owner: a.owner.toBase58(),
          data: a.data,
          lamports: Number(a.lamports),
          executable: false,
        });
    }
    const tables = (tx.message.addressTableLookups ?? []).map(
      (lookup) =>
        new AddressLookupTableAccount({
          key: lookup.accountKey,
          state: AddressLookupTableAccount.deserialize(
            Buffer.from(f.svm.getAccount(lookup.accountKey).data),
          ),
        }),
    );
    const keys = tx.message.getAccountKeys({
      addressLookupTableAccounts: tables,
    });
    for (let i = 0; i < keys.length; i++) {
      if (!tx.message.isAccountWritable(i)) continue;
      const key = keys.get(i),
        account = f.svm.getAccount(key);
      if (!account || account.lamports === 0n)
        await setAccount(key.toBase58(), {
          owner: "11111111111111111111111111111111",
          data: [],
          lamports: 0,
          executable: false,
        });
    }
    await setAccount(signer.publicKey.toBase58(), {
      owner: "11111111111111111111111111111111",
      data: [],
      lamports: 1_000_000_000,
      executable: false,
    });
    tx.message.recentBlockhash = (
      await rpc("getLatestBlockhash")
    ).value.blockhash;
    tx.sign([signer]);
    const wire = Buffer.from(tx.serialize()).toString("base64");
    const sim = await rpc("simulateTransaction", [
      wire,
      { encoding: "base64", sigVerify: true, commitment: "processed" },
    ]);
    assert.equal(
      sim.value.err,
      null,
      change +
        " " +
        JSON.stringify(sim.value.err) +
        " " +
        (sim.value.logs ?? []).slice(-8).join("\n"),
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
    assert.ok(status);
    assert.equal(status.err, null);
    const read = async (name, address) => {
      const a = (
        await rpc("getAccountInfo", [
          address.toBase58(),
          { encoding: "base64", commitment: "processed" },
        ])
      ).value;
      assert.ok(a);
      return codec(name).decode(Buffer.from(a.data[0], "base64").subarray(8));
    };
    const market = await read("Market", f.market),
      proposal = await read("OperatorProposal", f.proposal);
    assert.ok(market.operator.pending.equals(PublicKey.default));
    assert.ok(proposal.executed);
    if (change === "manager")
      assert.ok(market.operator.manager.equals(f.manager));
    else {
      const archive = await read(
        "OperatorRevenueAccount",
        pda(Buffer.from("operator_revenue"), f.market.toBuffer(), u64(0n)),
      );
      assert.ok(archive.sealed);
      assert.ok(archive.recipient.equals(f.payer));
      assert.ok(
        market.operator.revenue_recipient.equals(
          change === "handoff" ? f.next : f.recipient,
        ),
      );
      if (change === "handoff")
        assert.ok(market.operator.authority.equals(f.next));
    }
    rows.push({
      change,
      withLookupTable: withTable,
      signature,
      slot: status.slot,
      signer: signer.publicKey.toBase58(),
      signatureVerified: true,
      postStateVerified: true,
      status: "passed",
    });
    console.log(change, withTable, "signed execution passed");
  }
writeFileSync(
  process.env.SURFPOOL_REHEARSAL_PROOF ??
    "/private/tmp/dusk-signed-lifecycle-proof.json",
  JSON.stringify(
    {
      schemaVersion: 1,
      kind: "signed-offline-native-lifecycle-execution",
      genesis,
      protocolRevision: lock.revision,
      completeLiveReleaseMatrix: false,
      cases: rows,
    },
    null,
    2,
  ) + "\n",
  { mode: 0o600 },
);
