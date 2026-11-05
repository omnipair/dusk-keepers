import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { execFileSync, spawnSync } from "node:child_process";
import test from "node:test";
import { PublicKey, Transaction, TransactionInstruction, ComputeBudgetProgram } from "@solana/web3.js";
import { resolveLiquidationAccounts, liquidationInstruction } from "../packages/dusk-adapter/src/liquidation.ts";
import { bank } from "./helpers/liquidation-bank.ts";
import { withOracle, base, quote, provider } from "./helpers/oracle-fixture.ts";

const read = (path: string) => JSON.parse(readFileSync(path, "utf8"));
const layout = read("protocol/keeper-account-layout.v1.json");
const contract = read("protocol/keeper-instructions.v1.json");
const native = (f: any) => JSON.parse(execFileSync("target/debug/dusk-keeper", ["--flash-plan"], {
  input: JSON.stringify({ ...f, action: "health" }), encoding: "utf8",
}));

function fixture(format = 1, weight = 2500) {
  return withOracle(read("fixtures/litesvm/01-flash-BorrowPosition.json"), format, weight);
}

for (const format of [1, 2, 3, 4]) {
  test(`oracle format ${format}: Rust and TypeScript append the same read-only feeds`, async () => {
    const { f } = await fixture(format);
    const c = resolveLiquidationAccounts(layout, f.snapshot, f.position, f.buyer, f.kind, f.debtAsset);
    const expected = [base, ...([2, 4].includes(format) ? [quote] : [])].map(key => key.toBase58());
    assert.deepEqual(c.oracle, expected);
    const health = liquidationInstruction(contract, c, "preview_liquidation_health", { debtAsset: f.debtAsset });
    assert.deepEqual(native(f).instructions, [health]);
    for (const action of ["preview_flash_liquidation", "preview_emergency_liquidation"]) {
      const ix = liquidationInstruction(contract, c, action,
        action.includes("emergency")
          ? { debtAsset: f.debtAsset, collateralDebit: "1", full: false }
          : { debtAsset: f.debtAsset, maxRepayment: "1", full: false }, true);
      assert.deepEqual(ix.accounts.slice(-expected.length), expected.map(key => ({
        pubkey: Array.from(new PublicKey(key).toBytes()), is_signer: false, is_writable: false,
      })));
    }
  });
}

test("retained zero-weight oracle policy adds no feed dependency", async () => {
  const { f } = await fixture(4, 0);
  const c = resolveLiquidationAccounts(layout, f.snapshot, f.position, f.buyer, f.kind, f.debtAsset);
  assert.deepEqual(c.oracle, []);
  assert.deepEqual(native(f).instructions, [liquidationInstruction(contract, c, "preview_liquidation_health", { debtAsset: f.debtAsset })]);
});

test("unsupported positive-weight oracle policy fails closed in both planners", async () => {
  const { f } = await fixture(5);
  assert.throws(() => resolveLiquidationAccounts(layout, f.snapshot, f.position, f.buyer, f.kind, f.debtAsset), /oracle policy/);
  const result = spawnSync("target/debug/dusk-keeper", ["--flash-plan"], {
    input: JSON.stringify({ ...f, action: "health" }), encoding: "utf8",
  });
  assert.notEqual(result.status, 0);
});

test("deployed program authenticates the keeper's oracle feed and rejects missing, stale, or wrong-owner reports", async () => {
  const { f, state } = await fixture();
  const health = native(f).instructions[0];
  const svm = bank(f);
  const clock = svm.getClock();
  const report = Buffer.alloc(145);
  report.set(Buffer.from("DOPRV001"));
  report.fill(7, 8, 40);
  report.set(state.base_side.asset_mint.toBytes(), 40);
  report.set(state.quote_side.asset_mint.toBytes(), 72);
  report.writeBigUInt64LE(1_000_000_000n, 104);
  report.writeBigInt64LE(clock.unixTimestamp, 120);
  report.writeBigUInt64LE(clock.slot, 128);
  report.writeBigUInt64LE(1n, 136);
  report[144] = 1;
  const publish = (data: Buffer, owner = provider) => svm.setAccount(base, { data, owner, lamports: 10_000_000n, executable: false });
  const simulate = (omitFeed = false) => {
    const instruction = new TransactionInstruction({
      programId: new PublicKey(health.program_id), data: Buffer.from(health.data),
      keys: health.accounts.filter((a: any) => !omitFeed || !new PublicKey(a.pubkey).equals(base)).map((a: any) => ({
        pubkey: new PublicKey(a.pubkey), isSigner: a.is_signer, isWritable: a.is_writable,
      })),
    });
    const tx = new Transaction({ feePayer: new PublicKey(f.buyer), recentBlockhash: svm.latestBlockhash() }).add(
      ComputeBudgetProgram.requestHeapFrame({ bytes: 256 * 1024 }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }), instruction,
    );
    const result: any = svm.simulateTransaction(tx);
    return typeof result.err === "function" ? result : null;
  };
  publish(report);
  assert.equal(simulate(), null, "authenticated report must permit the native health preview");
  assert.ok(simulate(true), "omitted feed must reject");
  publish(report, quote);
  assert.ok(simulate(), "wrong owner must reject");
  const stale = Buffer.from(report);
  stale.writeBigInt64LE(clock.unixTimestamp - 61n, 120);
  publish(stale);
  assert.ok(simulate(), "stale report must reject");
});
