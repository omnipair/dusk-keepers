import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { LiteSVM } from "litesvm";
import {
  PublicKey,
  VersionedTransaction,
  Message,
  MessageV0,
} from "@solana/web3.js";
import { createTransferCheckedInstruction } from "@solana/spl-token";
import { BorshInstructionCoder } from "@coral-xyz/anchor";
import {
  decodeLiquidationQuote,
  resolveLiquidationAccounts,
} from "../../packages/dusk-adapter/src/liquidation.ts";
import {
  decodeMintFees,
  decodeClockEpoch,
  feeCredit,
} from "../../packages/dusk-adapter/src/transfer-fees.ts";
const lock = JSON.parse(readFileSync("protocol.lock.json", "utf8"));
const layout = JSON.parse(
  readFileSync("protocol/keeper-account-layout.v1.json", "utf8"),
);
const coder = new BorshInstructionCoder(
  JSON.parse(readFileSync("artifacts/dusk.json", "utf8")),
);
const clockKey = "SysvarC1ock11111111111111111111111111111111";
const planner = (f: any) =>
  JSON.parse(
    execFileSync("target/debug/dusk-keeper", ["--flash-plan"], {
      input: JSON.stringify(f),
      encoding: "utf8",
    }),
  );
export function bank(f: any) {
  const svm = new LiteSVM()
    .withSigverify(false)
    .withBlockhashCheck(false)
    .withTransactionHistory(0n);
  for (const p of lock.programs)
    svm.addProgramFromFile(new PublicKey(p.programId), p.binary.path);
  for (const [key, a] of Object.entries(f.bank) as Array<[string, any]>) {
    if (!a.executable)
      svm.setAccount(new PublicKey(key), {
        owner: new PublicKey(a.owner),
        data: Buffer.from(a.data),
        lamports: BigInt(a.lamports),
        executable: false,
      });
  }
  const raw = Buffer.from(f.bank[clockKey].data);
  const clock = svm.getClock();
  clock.slot = raw.readBigUInt64LE(0);
  clock.epochStartTimestamp = raw.readBigInt64LE(8);
  clock.epoch = raw.readBigUInt64LE(16);
  clock.leaderScheduleEpoch = raw.readBigUInt64LE(24);
  clock.unixTimestamp = raw.readBigInt64LE(32);
  svm.setClock(clock);
  return svm;
}
export function planWithTable(f: any, svm: LiteSVM) {
  let plan = planner(f);
  const keys = [
    ...new Set(
      plan.instructions.flatMap((ix: any) =>
        ix.accounts
          .filter((a: any) => !a.is_signer)
          .map((a: any) => new PublicKey(a.pubkey).toBase58()),
      ),
    ),
  ] as string[];
  const lookupTable = new PublicKey(
    createHash("sha256")
      .update(f.position + "keeper-fixture-lookup")
      .digest(),
  ).toBase58();
  const raw = Buffer.alloc(56 + keys.length * 32);
  raw.writeUInt32LE(1, 0);
  raw.writeBigUInt64LE(0xffffffffffffffffn, 4);
  raw.writeBigUInt64LE(0n, 12);
  keys.forEach((k, i) => new PublicKey(k).toBuffer().copy(raw, 56 + i * 32));
  svm.setAccount(new PublicKey(lookupTable), {
    owner: new PublicKey("AddressLookupTab1e1111111111111111111111111"),
    data: raw,
    lamports: 10000000n,
    executable: false,
  });
  plan = planner({ ...f, lookupTable, lookupAddresses: keys });
  const message = Buffer.from(plan.messageBase64, "base64");
  const wire = Buffer.concat([Buffer.from([1]), Buffer.alloc(64), message]);
  assert.ok(wire.length <= 1232, `wire ${wire.length}`);
  // Decode native compiled bytes using Solana's independent JS implementation.
  const tx = VersionedTransaction.deserialize(wire);
  assert.ok(tx.message instanceof MessageV0);
  for (const ix of plan.instructions.filter(
    (i: any) => new PublicKey(i.program_id).toBase58() === layout.programId,
  ))
    assert.ok(
      coder.decode(Buffer.from(ix.data)),
      "Anchor decodes native instruction",
    );
  return { tx, plan, lookupTable, lookupAddresses: keys };
}
export function result(svm: LiteSVM, tx: VersionedTransaction) {
  const value: any = svm.sendTransaction(tx);
  return typeof value.err === "function"
    ? { error: String(value.err()), logs: value.meta().logs() }
    : null;
}
export function payment(f: any, q: any, underpay = false) {
  const context = resolveLiquidationAccounts(
    layout,
    f.snapshot,
    f.position,
    f.buyer,
    f.kind,
    f.debtAsset,
  );
  const mint = f.snapshot.accounts[context.named.debt_mint];
  const fee = decodeMintFees(mint.owner, Buffer.from(mint.data));
  const clock = f.snapshot.accounts[clockKey];
  const epoch = decodeClockEpoch(
    clock.owner,
    Buffer.from(clock.data),
    f.snapshot.slot,
  );
  const net = BigInt(q.payment);
  let low = net,
    high = 0xffffffffffffffffn;
  while (low < high) {
    const mid = (low + high) / 2n;
    if (feeCredit(fee, epoch, mid, "effective") >= net) high = mid;
    else low = mid + 1n;
  }
  const ix = createTransferCheckedInstruction(
    new PublicKey(context.named.buyer_refund_account),
    new PublicKey(context.named.debt_mint),
    new PublicKey(context.named.repayment_vault),
    new PublicKey(f.buyer),
    underpay ? 0n : low,
    mint.data[44],
    [],
    new PublicKey(mint.owner),
  );
  return {
    program_id: Array.from(ix.programId.toBytes()),
    accounts: ix.keys.map((k) => ({
      pubkey: Array.from(k.pubkey.toBytes()),
      is_signer: k.isSigner,
      is_writable: k.isWritable,
    })),
    data: Array.from(ix.data),
  };
}
