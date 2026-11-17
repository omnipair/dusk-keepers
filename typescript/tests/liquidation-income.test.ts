import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { BorshAccountsCoder, BorshEventCoder } from "@coral-xyz/anchor";
import { PublicKey } from "@solana/web3.js";
import { bank, planWithTable, payment } from "./helpers/liquidation-bank.ts";
import { nativePlan } from "./helpers/native-planner.ts";
import { withOracle, base as oracleFeed } from "./helpers/oracle-fixture.ts";
import { buildFlashPlan, decodeLiquidationQuote, resolveLiquidationAccounts } from "../packages/dusk-adapter/src/liquidation.ts";

const read = (p: string) => JSON.parse(readFileSync(p, "utf8"));
const idl = read("artifacts/dusk.json");
const accounts = new BorshAccountsCoder(idl);
const events = new BorshEventCoder(idl);
const layout = read("protocol/keeper-account-layout.v1.json");
const contract = read("protocol/keeper-instructions.v1.json");
const settleAccounts = contract.instructions.find((i: any) => i.key === "dusk:settle_flash_liquidation").accounts;

function interestFixture() {
  const f = read("fixtures/litesvm/08-flash-LeveragePosition.json");
  const market = new PublicKey(f.snapshot.accounts[f.position].data.slice(40, 72)).toBase58();
  // Reclassify 10,000 atoms of indexed debt as accrued interest, consistently
  // for the position and aggregate isolated principal. Debt shares/index,
  // eligibility, custody, and the captured liquidation quote are unchanged.
  for (const [key, name] of [[f.position, "LeveragePosition"], [market, "Market"]]) {
    const state: any = accounts.decode(name, Buffer.from(f.bank[key].data));
    if (name === "LeveragePosition") state.debt_principal = state.debt_principal.subn(10_000);
    else state.debt.isolated_quote_principal = state.debt.isolated_quote_principal.subn(10_000);
    // Anchor's encode() buffer is smaller than Market; use its IDL layout.
    const bytes = Buffer.alloc(f.bank[key].data.length);
    bytes.set(accounts.accountDiscriminator(name));
    const length = (accounts as any).accountLayouts.get(name).layout.encode(state, bytes.subarray(8));
    assert.equal(length, bytes.length - 8);
    f.bank[key].data = Array.from(bytes);
    f.snapshot.accounts[key].data = Array.from(bytes);
  }
  const q = decodeLiquidationQuote("flash", Buffer.from(f.quoteBase64, "base64"));
  return { f: { ...f, internal: false, route: [payment(f, q)] }, q };
}

for (const prefix of [0, 5, 9]) {
  test(`inventory liquidation with interest and ${prefix} hLP settlement accounts`, () => {
    const { f, q } = interestFixture();
    const svm = bank(f);
    const { tx, plan } = planWithTable(f, svm);
    const settle = tx.message.compiledInstructions.at(-1)!;
    assert.equal(settle.accountKeyIndexes.length, settleAccounts.length + 9);
    // Exercise the original omission and the incomplete five-account fix.
    settle.accountKeyIndexes = settle.accountKeyIndexes.slice(0, settleAccounts.length + prefix);
    const balance = (name: string) => Buffer.from(svm.getAccount(new PublicKey(plan.named[name]))!.data).readBigUInt64LE(64);
    const debtBefore = balance("buyer_refund_account");
    const collateralBefore = balance("buyer_collateral_account");
    const positionBefore = Buffer.from(svm.getAccount(new PublicKey(f.position))!.data);
    const result: any = svm.sendTransaction(tx);
    if (prefix < 9) {
      assert.equal(typeof result.err, "function", "missing income accounts must reproduce the failure");
      assert.ok(result.meta().logs().some((l: string) => l.includes("Error Code: NotEnoughAccounts.")));
      assert.equal(balance("buyer_refund_account"), debtBefore);
      assert.equal(balance("buyer_collateral_account"), collateralBefore);
      assert.deepEqual(Buffer.from(svm.getAccount(new PublicKey(f.position))!.data), positionBefore);
      return;
    }
    assert.notEqual(typeof result.err, "function", result.toString());
    assert.equal(debtBefore - balance("buyer_refund_account"), 103_000n);
    assert.equal(balance("buyer_collateral_account") - collateralBefore, BigInt(q.collateralCredit));
    assert.equal(svm.getAccount(new PublicKey(plan.named.session))?.lamports ?? 0n, 0n);
    const decoded = result.innerInstructions().flatMap((g: any[]) => g).map((inner: any) => Buffer.from(inner.instruction().data()))
      .filter((data: Buffer) => data.subarray(0, 8).toString("hex") === "e445a52e51cb9a1d")
      .map((data: Buffer) => events.decode(data.subarray(8).toString("base64"))).filter(Boolean);
    const settled = decoded.find((e: any) => e.name === "FlashLiquidationSettled");
    assert.ok(settled, "settled event emitted");
    assert.equal(settled.data.loss.interest_paid.toString(), "1000");
    assert.ok(!decoded.some((e: any) => e.name === "SwapExecuted"), "inventory collateral is retained without a swap");
  });
}

for (const active of [false, true]) {
  test(`Rust/TypeScript inventory accounts preserve oracle suffix with active hLP=${active}`, () => {
    const file = active ? "08-flash-LeveragePosition.json" : "01-flash-BorrowPosition.json";
    const { f } = withOracle(read(`fixtures/litesvm/${file}`));
    const c = resolveLiquidationAccounts(layout, f.snapshot, f.position, f.buyer, f.kind, f.debtAsset);
    const q = decodeLiquidationQuote("flash", Buffer.from(f.quoteBase64, "base64"));
    const options = { internal: false, route: [payment(f, q)] };
    const native = nativePlan({ ...f, ...options });
    const ts = buildFlashPlan(contract, c, q, f.maxRepayment, options);
    assert.deepEqual(native.instructions, ts);
    const settle = ts.at(-1)!;
    const optionalReserve = settleAccounts.findIndex((a: any) => a.name === "collateral_reserve_vault");
    assert.equal(new PublicKey(settle.accounts[optionalReserve].pubkey).toBase58(), c.program, "external settlement keeps AMM reserve option absent");
    assert.equal(c.hlpIncome.length, active ? 9 : 0);
    assert.equal(settle.accounts.length, settleAccounts.length + c.hlpIncome.length + 1);
    assert.deepEqual(settle.accounts.at(-1), { pubkey: Array.from(oracleFeed.toBytes()), is_signer: false, is_writable: false });
    assert.deepEqual(settle.accounts.slice(settleAccounts.length, -1), c.hlpIncome.map(a => ({ pubkey: Array.from(new PublicKey(a.pubkey).toBytes()), is_signer: false, is_writable: a.writable })));
    const beginSpec = contract.instructions.find((i: any) => i.key === "dusk:begin_flash_liquidation");
    assert.equal(ts[2].accounts.length, beginSpec.accounts.length + 1, "begin retains only its oracle suffix");
  });
}
