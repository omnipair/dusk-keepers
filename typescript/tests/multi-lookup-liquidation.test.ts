import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { AddressLookupTableAccount, PublicKey, TransactionMessage, VersionedTransaction } from "@solana/web3.js";
import { nativePlan } from "./helpers/native-planner.ts";
import { bank, payment, result } from "./helpers/liquidation-bank.ts";
import { decodeLiquidationQuote } from "../packages/dusk-adapter/src/liquidation.ts";

test("native multi-table messages resolve duplicate keys once and execute external settlement", () => {
  const f = JSON.parse(readFileSync("fixtures/litesvm/07-flash-LeveragePosition.json", "utf8"));
  const q = decodeLiquidationQuote("flash", Buffer.from(f.quoteBase64, "base64"));
  const request = { ...f, internal: false, route: [payment(f, q)] };
  const original = nativePlan(request);
  const keys: string[] = [...new Set<string>(original.instructions.flatMap((i: any) => [
    new PublicKey(i.program_id).toBase58(), ...i.accounts.map((a: any) => new PublicKey(a.pubkey).toBase58()),
  ]))];
  const lookupTables = [0, 1].map((t) => ({
    address: new PublicKey(Buffer.alloc(32, 231 + t)).toBase58(),
    // Include a duplicate and a signer/program: only eligible keys may load.
    addresses: [f.buyer, keys[0], ...keys.filter((_, i) => i % 2 === t)],
  }));
  const plan = nativePlan({ ...request, lookupTables });
  const tx = VersionedTransaction.deserialize(Buffer.concat([Buffer.from([1]), Buffer.alloc(64), Buffer.from(plan.messageBase64, "base64")]));
  assert.equal(tx.message.addressTableLookups.length, 2);
  const tables = lookupTables.map((t) => new AddressLookupTableAccount({key: new PublicKey(t.address), state: {
    deactivationSlot: 0xffffffffffffffffn, lastExtendedSlot: 0, lastExtendedSlotStartIndex: 0, addresses: t.addresses.map(a => new PublicKey(a)),
  }}));
  const decoded = TransactionMessage.decompile(tx.message, { addressLookupTableAccounts: tables });
  assert.equal(decoded.instructions.length, plan.instructions.length);
  for (const [index, instruction] of decoded.instructions.entries()) {
    const expected = plan.instructions[index];
    assert.equal(instruction.programId.toBase58(), new PublicKey(expected.program_id).toBase58());
    assert.deepEqual(instruction.data, Buffer.from(expected.data));
    assert.deepEqual(instruction.keys.map(k => k.pubkey.toBase58()), expected.accounts.map((a: any) => new PublicKey(a.pubkey).toBase58()));
  }
  const loaded = tx.message.getAccountKeys({ addressLookupTableAccounts: tables });
  const resolved = Array.from({length: loaded.length}, (_, i) => loaded.get(i)!.toBase58());
  assert.equal(new Set(resolved).size, resolved.length);
  assert.equal(tx.message.header.numRequiredSignatures, 1);
  const svm = bank(f);
  for (const t of lookupTables) {
    const raw = Buffer.alloc(56 + 32 * t.addresses.length);
    raw.writeUInt32LE(1); raw.writeBigUInt64LE(0xffffffffffffffffn, 4);
    t.addresses.forEach((k, i) => new PublicKey(k).toBuffer().copy(raw, 56 + i * 32));
    svm.setAccount(new PublicKey(t.address), { owner: new PublicKey("AddressLookupTab1e1111111111111111111111111"), data: raw, lamports: 10_000_000n, executable: false });
  }
  assert.ok(tx.serialize().length <= 1232);
  assert.equal(result(svm, tx), null);
});
