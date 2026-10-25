import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync, readdirSync } from "node:fs";
import { PublicKey } from "@solana/web3.js";
import { bank, planWithTable, result } from "./helpers/liquidation-bank.ts";

const amount = (data: Uint8Array) => Buffer.from(data).readBigUInt64LE(64);

for (const file of readdirSync("fixtures/hlp-litesvm").filter((entry) =>
  entry.endsWith(".json"),
)) {
  const fixture = JSON.parse(
    readFileSync(`fixtures/hlp-litesvm/${file}`, "utf8"),
  );

  test(`native keeper executes pinned hLP recovery: ${file}`, () => {
    const svm = bank(fixture);
    const market = new PublicKey(fixture.market);
    const marketBefore = Buffer.from(svm.getAccount(market)!.data);
    const { tx, plan } = planWithTable(fixture, svm);
    const debt = new PublicKey(plan.named.buyer_refund_account);
    const collateral = new PublicKey(plan.named.buyer_collateral_account);
    const debtBefore = amount(svm.getAccount(debt)!.data);
    const collateralBefore = amount(svm.getAccount(collateral)!.data);

    const failure = result(svm, tx);
    assert.equal(failure, null, JSON.stringify(failure));
    assert.notDeepEqual(Buffer.from(svm.getAccount(market)!.data), marketBefore);

    if (fixture.action === "hlp_flash") {
      assert.equal(
        svm.getAccount(new PublicKey(plan.named.session))?.lamports ?? 0n,
        0n,
        "flash session must close",
      );
      assert.equal(
        amount(svm.getAccount(new PublicKey(plan.named.repayment_vault))!.data),
        0n,
        "isolated payment vault must finish empty",
      );
      assert.ok(amount(svm.getAccount(debt)!.data) < debtBefore);
      assert.ok(amount(svm.getAccount(collateral)!.data) > collateralBefore);
    } else {
      assert.ok(
        amount(svm.getAccount(new PublicKey(plan.named.caller_bounty_account))!.data) >
          debtBefore,
        "terminal caller receives the bounded bounty",
      );
    }
  });

  if (fixture.action === "hlp_flash")
    test(`hLP flash underpayment rolls back atomically: ${file}`, () => {
      const svm = bank(fixture);
      const market = new PublicKey(fixture.market);
      const before = Buffer.from(svm.getAccount(market)!.data);
      const { tx } = planWithTable({ ...fixture, underpay: true }, svm);
      assert.ok(result(svm, tx), "underpayment must reject");
      assert.deepEqual(Buffer.from(svm.getAccount(market)!.data), before);
    });
}
