import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { BorshEventCoder } from "@coral-xyz/anchor";
import { PublicKey, Transaction } from "@solana/web3.js";
import { TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID, createTransferCheckedWithFeeInstruction } from "@solana/spl-token";
import bs58 from "bs58";
import { bank, payment, planWithTable } from "./helpers/liquidation-bank.ts";
import { decodeLiquidationQuote } from "../packages/dusk-adapter/src/liquidation.ts";

const idl = JSON.parse(readFileSync("artifacts/dusk.json", "utf8"));
const coder = new BorshEventCoder(idl);
const field = (a: any, name: string) => typeof a[name] === "function" ? a[name]() : a[name];
const key = (value: number) => new PublicKey(Buffer.alloc(32, value));

function report(input: any, env: NodeJS.ProcessEnv = {}) {
  const result = spawnSync("target/debug/dusk-keeper", ["--receipt-log"], {
    input: JSON.stringify(input), encoding: "utf8",
    env: { PATH: process.env.PATH, KEEPER_LOG_COLOR: "never", ...env },
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}

function executionReceipt(file: string) {
  const f = JSON.parse(readFileSync(`fixtures/litesvm/${file}`, "utf8"));
  const svm = bank(f);
  const q = decodeLiquidationQuote(f.action, Buffer.from(f.quoteBase64, "base64"));
  const request = f.action === "flash" ? { ...f, internal: false, route: [payment(f, q)] } : f;
  const { tx, lookupAddresses } = planWithTable(request, svm);
  const loaded = { writable: [] as string[], readonly: [] as string[] };
  for (const table of tx.message.addressTableLookups) {
    loaded.writable.push(...Array.from(table.writableIndexes).map(i => lookupAddresses[i]));
    loaded.readonly.push(...Array.from(table.readonlyIndexes).map(i => lookupAddresses[i]));
  }
  const addresses = [...tx.message.staticAccountKeys.map(k => k.toBase58()), ...loaded.writable, ...loaded.readonly];
  const pre = addresses.map(a => svm.getAccount(new PublicKey(a)));
  const outcome: any = svm.simulateTransaction(tx);
  assert.notEqual(typeof outcome.err, "function", String(outcome));
  const meta = outcome.meta();
  const posts = new Map<string, any>(outcome.postAccounts().map(([k, a]: any) => [new PublicKey(k).toBase58(), a]));
  const post = addresses.map((a, i) => posts.get(a) ?? pre[i]);
  const tokenBalances = (accounts: any[]) => accounts.flatMap((a, accountIndex) => {
    if (!a) return [];
    const data = Buffer.from(field(a, "data"));
    const programId = new PublicKey(field(a, "owner")).toBase58();
    if (![TOKEN_PROGRAM_ID.toBase58(), TOKEN_2022_PROGRAM_ID.toBase58()].includes(programId) || data.length < 165 || ![1, 2].includes(data[108])) return [];
    const mint = new PublicKey(data.subarray(0, 32));
    const mintAccount = svm.getAccount(mint)!;
    return [{ accountIndex, mint: mint.toBase58(), owner: new PublicKey(data.subarray(32, 64)).toBase58(), programId,
      uiTokenAmount: { amount: data.readBigUInt64LE(64).toString(), decimals: Buffer.from(field(mintAccount, "data"))[44] } }];
  });
  const innerInstructions = meta.innerInstructions().map((group: any[], index: number) => ({ index,
    instructions: group.map(inner => { const ix = inner.instruction(); return { programIdIndex: ix.programIdIndex(), accounts: Array.from(ix.accounts()), data: bs58.encode(ix.data()), stackHeight: inner.stackHeight() }; }),
  }));
  const input = { action: f.action, position: f.position, market: "fixture market", signature: bs58.encode(Buffer.alloc(64, 3)), receipt: {
    slot: f.snapshot.slot, transaction: [Buffer.from(tx.serialize()).toString("base64"), "base64"],
    meta: { err: null, fee: 5000, computeUnitsConsumed: Number(meta.computeUnitsConsumed()), loadedAddresses: loaded,
      preBalances: pre.map(a => a ? Number(field(a, "lamports")) : 0), postBalances: post.map(a => a ? Number(field(a, "lamports")) : 0),
      preTokenBalances: tokenBalances(pre), postTokenBalances: tokenBalances(post), innerInstructions, logMessages: meta.logs() },
  } };
  return { input, addresses, f };
}

test("finalized emergency receipts show actual rewards, swaps, costs and token movements", () => {
  const { input, addresses } = executionReceipt("03-emergency-LeveragePosition.json");
  const events = input.receipt.meta.innerInstructions.flatMap((g: any) => g.instructions)
    .filter((ix: any) => addresses[ix.programIdIndex] === idl.address)
    .map((ix: any) => Buffer.from(bs58.decode(ix.data)))
    .filter((data: Buffer) => data.subarray(0, 8).toString("hex") === "e445a52e51cb9a1d")
    .map((data: Buffer) => coder.decode(data.subarray(8).toString("base64"))).filter(Boolean);
  assert.ok(events.some((e: any) => e.name === "EmergencyLiquidationSettled"));
  assert.ok(events.some((e: any) => e.name === "SwapExecuted"));
  const output = report(input);
  assert.match(output, /FINALIZED SUCCESS/);
  assert.match(output, /SOL fee paid: 0\.000005000 SOL/);
  assert.match(output, /REWARD RECEIVED: 0\.000018/);
  assert.match(output, /SWAP EXECUTED: Dusk/);
  assert.match(output, /Debt repaid from swap proceeds:/);
  assert.match(output, /NET RECEIVED \+0\.000018/);
  assert.match(output, /TOKEN ACCOUNT CHANGES/);
  assert.match(output, /collateral_vault/);
  assert.ok(!output.includes("unavailable"), output);
  assert.ok(!output.includes("\x1b["));
});

test("flash receipts distinguish keeper inventory payment from protocol debt repayment", () => {
  const { input } = executionReceipt("01-flash-BorrowPosition.json");
  const output = report(input);
  assert.match(output, /FLASH LIQUIDATION/);
  assert.match(output, /NET PAID -/);
  assert.match(output, /NET RECEIVED \+/);
  assert.match(output, /repayment_vault/);
  assert.match(output, /Debt repaid:/);
  assert.ok(!output.includes("Event details unavailable"), output);
});

test("failed transactions report the fee without claiming rolled-back rewards or swaps", () => {
  const { input } = executionReceipt("03-emergency-LeveragePosition.json");
  input.receipt.meta.err = { InstructionError: [2, { Custom: 6033 }] } as any;
  input.receipt.meta.postTokenBalances = structuredClone(input.receipt.meta.preTokenBalances);
  const output = report(input);
  assert.match(output, /FINALIZED FAILED/);
  assert.match(output, /SOL fee paid: 0\.000005000/);
  assert.match(output, /rolled back/);
  assert.ok(!output.includes("REWARD RECEIVED"));
  assert.ok(!output.includes("SWAP EXECUTED"));
  assert.ok(!output.includes("NET RECEIVED"));
});

test("receipt colors are optional and NO_COLOR overrides forced color", () => {
  const { input } = executionReceipt("03-emergency-LeveragePosition.json");
  assert.match(report(input, { KEEPER_LOG_COLOR: "always" }), /\x1b\[32m/);
  assert.ok(!report(input, { KEEPER_LOG_COLOR: "always", NO_COLOR: "1" }).includes("\x1b["));
});

test("events emitted by another program cannot claim a Dusk reward", () => {
  const { input, addresses } = executionReceipt("03-emergency-LeveragePosition.json");
  for (const group of input.receipt.meta.innerInstructions) for (const ix of group.instructions) {
    if (addresses[ix.programIdIndex] === idl.address) ix.programIdIndex = addresses.indexOf(TOKEN_PROGRAM_ID.toBase58());
  }
  const output = report(input);
  assert.ok(!output.includes("REWARD RECEIVED"));
  assert.ok(!output.includes("SWAP EXECUTED"));
  assert.match(output, /NET RECEIVED/);
});

test("missing v0 lookup addresses are rejected instead of mislabeling accounts", () => {
  const { input } = executionReceipt("03-emergency-LeveragePosition.json");
  delete (input.receipt.meta as any).loadedAddresses;
  const result = spawnSync("target/debug/dusk-keeper", ["--receipt-log"], { input: JSON.stringify(input), encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /loaded lookup addresses/);
});

test("missing token balance metadata is unavailable rather than zero", () => {
  const { input } = executionReceipt("03-emergency-LeveragePosition.json");
  delete (input.receipt.meta as any).preTokenBalances;
  const output = report(input);
  assert.match(output, /Token balance details unavailable; no zero balances assumed/);
  assert.ok(!output.includes("NET RECEIVED"));
});

test("changed token authority cannot be misreported as a keeper payment", () => {
  const { input } = executionReceipt("03-emergency-LeveragePosition.json");
  input.receipt.meta.postTokenBalances[0].owner = key(50).toBase58();
  const output = report(input);
  assert.match(output, /Token balance details unavailable/);
  assert.ok(!output.includes("NET RECEIVED"));
  assert.ok(!output.includes("NET PAID"));
});

test("legacy Token-2022 receipts preserve large integer amounts, transfer fees and new accounts", () => {
  const payer = key(40), source = key(41), destination = key(42), mint = key(43);
  const amount = 9_007_199_254_740_993n;
  const instruction = createTransferCheckedWithFeeInstruction(source, mint, destination, payer, amount, 9, 7n);
  const transaction = new Transaction({ feePayer: payer, recentBlockhash: key(44).toBase58() }).add(instruction);
  const message = transaction.compileMessage();
  const addresses = message.accountKeys.map(k => k.toBase58());
  const token = (account: PublicKey, owner: PublicKey, amount: bigint) => ({ accountIndex: addresses.indexOf(account.toBase58()), mint: mint.toBase58(), owner: owner.toBase58(), uiTokenAmount: { amount: amount.toString(), decimals: 9 } });
  const input = { signature: "legacy-fixture", action: "transfer", receipt: {
    slot: 12, transaction: [transaction.serialize({ requireAllSignatures: false, verifySignatures: false }).toString("base64"), "base64"],
    meta: { err: null, fee: 5000, preBalances: addresses.map(() => 100000000), postBalances: addresses.map((_, i) => i === 0 ? 99995000 : 100000000), innerInstructions: [],
      preTokenBalances: [token(source, payer, amount)], postTokenBalances: [token(source, payer, 0n), token(destination, key(45), amount - 7n)] },
  } };
  const output = report(input);
  assert.match(output, /TRANSFER 9007199\.254740993/);
  assert.match(output, /Explicit transfer fee: 0\.000000007/);
  assert.match(output, /0\.000000000 -> 9007199\.254740986/);
  assert.match(output, /NET PAID -9007199\.254740993/);
});
