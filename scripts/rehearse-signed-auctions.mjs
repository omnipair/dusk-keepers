// Execute bounded native continuous-auction settlements on an isolated local validator.
// Public synthetic banks and disposable keys; never enables hosted live signing.
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
const { auctionFixture } = await import(
  pathToFileURL(root + "/typescript/tests/helpers/auction-bank.ts")
);
const { run } = await import(
  pathToFileURL(root + "/typescript/tests/helpers/lifecycle-bank.ts")
);
const idl = JSON.parse(readFileSync(root + "/artifacts/dusk.json", "utf8"));
const errorCode = (name) => idl.errors.find((e) => e.name === name).code;
const rows = [];
const cases = [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID]
  .flatMap((soldProgram) =>
    [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID].map((paymentProgram) => ({
      soldProgram,
      paymentProgram,
      transferFeeBps: 0,
    })),
  )
  .concat(
    [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID].map((paymentProgram) => ({
      soldProgram: TOKEN_2022_PROGRAM_ID,
      paymentProgram,
      transferFeeBps: 100,
    })),
  );
for (const { soldProgram, paymentProgram, transferFeeBps } of cases) {
  const signer = Keypair.generate(),
    f = auctionFixture(
      soldProgram,
      paymentProgram,
      signer.publicKey,
      transferFeeBps,
    );
  const checked = await run(f, signer.publicKey, undefined, {
    KEEPER_PROFILE: "auction-arbitrage",
    KEEPER_AUCTION_MAX_SOLD: "1000",
    KEEPER_AUCTION_MAX_PAYMENT: "1000000",
    KEEPER_AUCTION_SLIPPAGE_BPS: "0",
  });
  assert.equal(checked.simulations.length, 8, checked.output);
  assert.ok(checked.simulations.every((s) => s.error === null));
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
  await setAccount(signer.publicKey.toBase58(), {
    owner: "11111111111111111111111111111111",
    data: [],
    lamports: 1000000000,
    executable: false,
  });
  const account = async (address) =>
    (
      await rpc("getAccountInfo", [
        address,
        { encoding: "base64", commitment: "processed" },
      ])
    ).value;
  const amount = async (address) => {
    const a = await account(address);
    assert.ok(a);
    return Buffer.from(a.data[0], "base64").readBigUInt64LE(64);
  };
  for (let index = 0; index < checked.simulations.length; index++) {
    const original = checked.simulations[index].transaction,
      compiled = original.message.compiledInstructions[0],
      keys = original.message.getAccountKeys(),
      raw = Buffer.from(compiled.data);
    const named = (name) =>
      keys
        .get(
          compiled.accountKeyIndexes[
            idl.instructions
              .find((i) => i.name === "settle_protocol_auction")
              .accounts.findIndex((a) => a.name === name)
          ],
        )
        .toBase58();
    const payment = named("bidder_payment_account"),
      receive = named("bidder_receive_account"),
      vault = named("sold_vault");
    const outcome = f.svm.simulateTransaction(original);
    assert.equal(typeof outcome.err, "undefined");
    const post = outcome
      .postAccounts()
      .find(([key]) => new PublicKey(key).toBase58() === payment)[1];
    const expected = f.svm.getAccount(new PublicKey(payment)).data;
    const paid =
      Buffer.from(expected).readBigUInt64LE(64) -
      Buffer.from(
        typeof post.data === "function" ? post.data() : post.data,
      ).readBigUInt64LE(64);
    assert.ok(paid > 0n && paid < 1000000n);
    const build = async (cap, repeat = false) => {
      const bytes = Buffer.from(raw);
      bytes.writeBigUInt64LE(cap, 18);
      const ix = new TransactionInstruction({
        programId: keys.get(compiled.programIdIndex),
        data: bytes,
        keys: [...compiled.accountKeyIndexes].map((i) => ({
          pubkey: keys.get(i),
          isSigner: original.message.isAccountSigner(i),
          isWritable: original.message.isAccountWritable(i),
        })),
      });
      const tx = new VersionedTransaction(
        new TransactionMessage({
          payerKey: signer.publicKey,
          recentBlockhash: (await rpc("getLatestBlockhash")).value.blockhash,
          instructions: repeat
            ? [ComputeBudgetProgram.setComputeUnitLimit({ units: 1400000 }), ix]
            : [ix],
        }).compileToLegacyMessage(),
      );
      tx.sign([signer]);
      assert.ok(tx.serialize().length <= 1232);
      return Buffer.from(tx.serialize()).toString("base64");
    };
    const simulate = (wire) =>
      rpc("simulateTransaction", [
        wire,
        { encoding: "base64", sigVerify: true, commitment: "processed" },
      ]);
    const underpaid = await simulate(await build(paid - 1n));
    assert.equal(
      underpaid.value.err?.InstructionError?.[1]?.Custom,
      errorCode("InsufficientAuctionPayment"),
    );
    let inventoryChecked = false;
    if (index === 0) {
      const before = await account(payment),
        bytes = Buffer.from(before.data[0], "base64");
      bytes.writeBigUInt64LE(0n, 64);
      await setAccount(payment, {
        owner: before.owner,
        data: bytes,
        lamports: before.lamports,
        executable: false,
      });
      const missing = await simulate(await build(paid));
      assert.equal(
        missing.value.err?.InstructionError?.[1]?.Custom,
        errorCode("InsufficientBalance"),
      );
      await setAccount(payment, {
        owner: before.owner,
        data: Buffer.from(before.data[0], "base64"),
        lamports: before.lamports,
        executable: false,
      });
      inventoryChecked = true;
    }
    const beforePayment = await amount(payment),
      beforeReceive = await amount(receive),
      beforeVault = await amount(vault);
    const recipients = [
      ...new Set([
        named("treasury_payment_account"),
        named("staking_vault_payment_account"),
      ]),
    ];
    const beforeRecipients = await Promise.all(recipients.map(amount));
    const wire = await build(paid),
      simulated = await simulate(wire);
    assert.equal(simulated.value.err, null, JSON.stringify(simulated.value));
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
    assert.equal(beforePayment - (await amount(payment)), paid);
    assert.equal(
      (await amount(receive)) - beforeReceive,
      1000n - (1000n * BigInt(transferFeeBps) + 9999n) / 10000n,
    );
    assert.equal(beforeVault - (await amount(vault)), 1000n);
    const afterRecipients = await Promise.all(recipients.map(amount));
    assert.equal(
      afterRecipients.reduce((sum, v, i) => sum + v - beforeRecipients[i], 0n),
      paid,
    );
    const race = await simulate(await build(paid, true));
    assert.equal(
      race.value.err?.InstructionError?.[1]?.Custom,
      errorCode("UnbackedFeeLiability"),
    );
    assert.equal(await amount(payment), beforePayment - paid);
    rows.push({
      soldProgram: soldProgram.toBase58(),
      paymentProgram: paymentProgram.toBase58(),
      lane: raw[8] === 0 ? "fee" : "buyback",
      source: raw[9] === 0 ? "swap" : "interest",
      soldMint: named("sold_mint"),
      signature,
      slot: status.slot,
      signer: signer.publicKey.toBase58(),
      soldAmount: "1000",
      transferFeeBps,
      netSoldAmount: (
        1000n -
        (1000n * BigInt(transferFeeBps) + 9999n) / 10000n
      ).toString(),
      transferFeeBps,
      maxPaymentAmount: paid.toString(),
      signatureVerified: true,
      paymentBoundEnforced: true,
      inventoryChecked,
      competingSettlementRejected: true,
      custodyAndRecipientsVerified: true,
      status: "passed",
    });
  }
  console.log(
    soldProgram.toBase58(),
    paymentProgram.toBase58(),
    "eight bounded signed auctions passed",
  );
}
writeFileSync(
  process.env.SURFPOOL_REHEARSAL_PROOF ??
    "/private/tmp/dusk-signed-auction-proof.json",
  JSON.stringify(
    {
      schemaVersion: 1,
      kind: "signed-offline-native-continuous-auction-execution",
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
