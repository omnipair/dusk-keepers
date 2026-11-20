// Execute captured TP/SL protection orders with actual signatures on an isolated local validator.
// This covers partial/full closes, not the entire live-release matrix.
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
  ComputeBudgetProgram,
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
const dusk = JSON.parse(readFileSync(root + "/artifacts/dusk.json", "utf8"));
const { IdlCoder } = require("@coral-xyz/anchor/dist/cjs/coder/borsh/idl.js");
function decode(idl, name, raw) {
  return IdlCoder.typeDefLayout({
    typeDef: idl.types.find((t) => t.name === name),
    types: idl.types,
  }).decode(Buffer.from(raw).subarray(8));
}
const rows = [];
for (const file of readdirSync(root + "/fixtures/orders-signed")
  .filter((f) => f.endsWith(".json"))
  .sort()) {
  const original = JSON.parse(
    readFileSync(root + "/fixtures/orders-signed/" + file),
  );
  assert.equal(original.sourceCommit, lock.source.gitCommit);
  const signer = Keypair.generate(),
    buyer = signer.publicKey.toBase58(),
    oldBuyer = new PublicKey(original.buyer).toBuffer(),
    names = new Map([[original.buyer, buyer]]),
    owned = new Set();
  for (const [address, a] of Object.entries(original.bank))
    if (
      [TOKEN_PROGRAM_ID.toBase58(), TOKEN_2022_PROGRAM_ID.toBase58()].includes(
        a.owner,
      ) &&
      a.data.length >= 165 &&
      Buffer.from(a.data).subarray(32, 64).equals(oldBuyer)
    ) {
      const mint = new PublicKey(Buffer.from(a.data).subarray(0, 32));
      const next = getAssociatedTokenAddressSync(
        mint,
        signer.publicKey,
        false,
        new PublicKey(a.owner),
      ).toBase58();
      assert.ok(!owned.has(next));
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
              Object.entries(v).map(([k, val]) => [
                names.get(k) ?? k,
                rewrite(val),
              ]),
            )
          : v;
  const f = rewrite(original);
  for (const accounts of [f.bank, f.snapshot.accounts])
    for (const [address, a] of Object.entries(accounts)) {
      if (!a || !owned.has(address)) continue;
      const raw = Buffer.from(a.data);
      signer.publicKey.toBuffer().copy(raw, 32);
      a.data = [...raw];
    }
  const clock = Buffer.from(
    f.bank.SysvarC1ock11111111111111111111111111111111.data,
  );
  const current = await rpc("getSlot", [{ commitment: "processed" }]);
  await rpc("surfnet_timeTravel", [
    { absoluteSlot: Math.max(Number(clock.readBigUInt64LE()), current + 100) },
  ]);
  for (const [address, a] of Object.entries(f.bank))
    if (!a.executable) await setAccount(address, a);
  await setAccount(buyer, {
    owner: "11111111111111111111111111111111",
    data: [],
    lamports: 1_000_000_000,
    executable: false,
  });
  const request = {
    order: f.order,
    kind: f.kind,
    executor: buyer,
    snapshot: f.snapshot,
  };
  let plan = nativePlan(request, "--order-plan");
  for (const key of Object.values(plan.named)) {
    const address = new PublicKey(key).toBase58();
    if (!(address in request.snapshot.accounts))
      request.snapshot.accounts[address] = null;
  }
  plan = nativePlan(request, "--order-plan");
  const rewardAddress = new PublicKey(plan.reward_account).toBase58();
  const beforeReward = f.bank[rewardAddress]
    ? Buffer.from(f.bank[rewardAddress].data).readBigUInt64LE(64)
    : 0n;
  const instructions = plan.instructions.map(
    (ix) =>
      new TransactionInstruction({
        programId: new PublicKey(ix.program_id),
        data: Buffer.from(ix.data),
        keys: ix.accounts.map((a) => ({
          pubkey: new PublicKey(a.pubkey),
          isSigner: a.is_signer,
          isWritable: a.is_writable,
        })),
      }),
  );
  const addresses = [
    ...new Map(
      instructions
        .flatMap((ix) => ix.keys)
        .filter((a) => !a.isSigner)
        .map((a) => [a.pubkey.toBase58(), a.pubkey]),
    ).values(),
  ];
  const table = new PublicKey(
    createHash("sha256")
      .update(file + buyer)
      .digest(),
  );
  const data = Buffer.alloc(56 + addresses.length * 32);
  data.writeUInt32LE(1);
  data.writeBigUInt64LE(0xffffffffffffffffn, 4);
  addresses.forEach((key, i) => key.toBuffer().copy(data, 56 + i * 32));
  await setAccount(table.toBase58(), {
    owner: "AddressLookupTab1e1111111111111111111111111",
    executable: false,
    data,
  });
  const lookup = new AddressLookupTableAccount({
    key: table,
    state: {
      deactivationSlot: 0xffffffffffffffffn,
      lastExtendedSlot: 0,
      lastExtendedSlotStartIndex: 0,
      authority: undefined,
      addresses,
    },
  });
  const tx = new VersionedTransaction(
    new TransactionMessage({
      payerKey: signer.publicKey,
      recentBlockhash: (await rpc("getLatestBlockhash")).value.blockhash,
      instructions,
    }).compileToV0Message([lookup]),
  );
  tx.sign([signer]);
  assert.ok(tx.serialize().length <= 1232);
  const wire = Buffer.from(tx.serialize()).toString("base64"),
    forged = Buffer.from(tx.serialize());
  forged[1] ^= 1;
  assert.equal(
    (
      await rpc("simulateTransaction", [
        forged.toString("base64"),
        { encoding: "base64", sigVerify: true, commitment: "processed" },
      ])
    ).value.err,
    "SignatureFailure",
  );
  const sim = await rpc("simulateTransaction", [
    wire,
    { encoding: "base64", sigVerify: true, commitment: "processed" },
  ]);
  assert.equal(
    sim.value.err,
    null,
    file +
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
  const order = (
    await rpc("getAccountInfo", [
      f.order,
      { encoding: "base64", commitment: "processed" },
    ])
  ).value;
  assert.ok(order === null || order.lamports === 0, "order closes");
  const after = (
    await rpc("getAccountInfo", [
      f.position,
      { encoding: "base64", commitment: "processed" },
    ])
  ).value;
  if (f.closeBps === 10000)
    assert.ok(after === null || after.lamports === 0, "position fully closes");
  else {
    assert.ok(after && after.lamports > 0);
    const beforePosition = decode(
        dusk,
        "LeveragePosition",
        Buffer.from(f.bank[f.position].data),
      ),
      afterPosition = decode(
        dusk,
        "LeveragePosition",
        Buffer.from(after.data[0], "base64"),
      );
    assert.ok(
      afterPosition.collateral_amount.lt(beforePosition.collateral_amount),
      "partial close reduces collateral",
    );
  }
  const reward = async () => {
    const a = (
      await rpc("getAccountInfo", [
        rewardAddress,
        { encoding: "base64", commitment: "processed" },
      ])
    ).value;
    return a ? Buffer.from(a.data[0], "base64").readBigUInt64LE(64) : 0n;
  };
  const paid = await reward();
  assert.ok(paid > beforeReward, "executor receives the fee");
  const repeatTx = new VersionedTransaction(
    new TransactionMessage({
      payerKey: signer.publicKey,
      recentBlockhash: (await rpc("getLatestBlockhash")).value.blockhash,
      // Distinct message: the validator must execute the already-resolved order
      // check rather than return its duplicate-signature cache entry.
      instructions: [
        ...instructions.map((ix) =>
          ix.programId.equals(ComputeBudgetProgram.programId) &&
          ix.data[0] === 2
            ? ComputeBudgetProgram.setComputeUnitLimit({
                units:
                  ix.data.readUInt32LE(1) === 1_399_999 ? 1_399_998 : 1_399_999,
              })
            : ix,
        ),
      ],
    }).compileToV0Message([lookup]),
  );
  repeatTx.sign([signer]);
  const repeated = await rpc("simulateTransaction", [
    Buffer.from(repeatTx.serialize()).toString("base64"),
    { encoding: "base64", sigVerify: true, commitment: "processed" },
  ]);
  assert.notEqual(
    repeated.value.err,
    null,
    "resolved order cannot execute twice",
  );
  assert.ok(
    (repeated.value.logs ?? []).some((line) =>
      lock.programs.some((program) =>
        line.includes(program.programId + " invoke"),
      ),
    ),
    "repeat reaches the program with a valid signature: " +
      JSON.stringify(repeated.value),
  );
  assert.equal(
    repeated.value.err.InstructionError?.[1]?.Custom,
    3012,
    "resolved position or order is no longer initialized",
  );
  assert.equal(await reward(), paid, "repeat pays no second fee");
  rows.push({
    case: file,
    sourceCommit: f.sourceCommit,
    signature,
    slot: status.slot,
    signer: buyer,
    signatureVerified: true,
    forgedSignatureRejected: true,
    repeatRejected: true,
    executorFeeCredit: (paid - beforeReward).toString(),
    orderClosed: true,
    positionClosed: f.closeBps === 10000,
    status: "passed",
  });
  console.log(file + " signed close passed");
}
const proof = {
  schemaVersion: 1,
  kind: "signed-offline-native-order-execution",
  genesis,
  protocolRevision: lock.revision,
  completeLiveReleaseMatrix: false,
  cases: rows,
};
writeFileSync(
  process.env.SURFPOOL_REHEARSAL_PROOF ??
    "/private/tmp/dusk-signed-order-proof.json",
  JSON.stringify(proof, null, 2) + "\n",
  { mode: 0o600 },
);
