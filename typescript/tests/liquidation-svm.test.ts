import assert from "node:assert/strict";
import test from "node:test";
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
} from "../packages/dusk-adapter/src/liquidation.ts";
import {
  decodeMintFees,
  decodeClockEpoch,
  feeCredit,
} from "../packages/dusk-adapter/src/transfer-fees.ts";
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
import {
  bank,
  planWithTable,
  result,
  payment,
} from "./helpers/liquidation-bank.ts";
for (const file of readdirSync("fixtures/litesvm").filter((f) =>
  f.endsWith(".json"),
)) {
  const f = JSON.parse(readFileSync(`fixtures/litesvm/${file}`, "utf8"));
  test(`native keeper executes against pinned SVM: ${file}`, () => {
    assert.equal(f.sourceCommit, lock.source.gitCommit);
    const q = decodeLiquidationQuote(
      f.action,
      Buffer.from(f.quoteBase64, "base64"),
    );
    const svm = bank(f);
    const request =
      f.action === "flash"
        ? { ...f, internal: false, route: [payment(f, q)] }
        : f;
    const { tx, plan } = planWithTable(request, svm);
    const failure = result(svm, tx);
    assert.equal(failure, null, JSON.stringify(failure));
    if (f.action === "flash")
      assert.equal(
        svm.getAccount(new PublicKey(plan.named.session))?.lamports ?? 0n,
        0n,
        "session closed",
      );
  });
  if (f.action === "flash")
    test(`native pair rolls back underpayment: ${file}`, () => {
      const q = decodeLiquidationQuote(
        "flash",
        Buffer.from(f.quoteBase64, "base64"),
      );
      const svm = bank(f);
      const before = Buffer.from(
        svm.getAccount(new PublicKey(f.position))!.data,
      );
      const { tx } = planWithTable(
        { ...f, internal: false, route: [payment(f, q, true)] },
        svm,
      );
      assert.ok(result(svm, tx), "underpayment must reject");
      assert.deepEqual(
        Buffer.from(svm.getAccount(new PublicKey(f.position))!.data),
        before,
      );
    });
}

// The dedicated #45 internal-netting fixture: no wallet debt payment and no
// external route. Both legs must be present in a single packet-sized transaction.
test("native internal Dusk flash settlement repays and releases its session", () => {
  const f = JSON.parse(
    readFileSync("fixtures/litesvm/07-flash-LeveragePosition.json", "utf8"),
  );
  const svm = bank(f);
  const { tx, plan } = planWithTable({ ...f, internal: true, route: [] }, svm);
  assert.equal(result(svm, tx), null);
  assert.equal(
    svm.getAccount(new PublicKey(plan.named.session))?.lamports ?? 0,
    0,
  );
});
test("missing settle rolls back flashed collateral and debt", () => {
  const f = JSON.parse(
    readFileSync("fixtures/litesvm/07-flash-LeveragePosition.json", "utf8"),
  );
  const svm = bank(f);
  const { tx } = planWithTable({ ...f, internal: true, route: [] }, svm);
  const before = Buffer.from(svm.getAccount(new PublicKey(f.position))!.data);
  tx.message.compiledInstructions.pop();
  assert.ok(result(svm, tx));
  assert.deepEqual(
    Buffer.from(svm.getAccount(new PublicKey(f.position))!.data),
    before,
  );
});
