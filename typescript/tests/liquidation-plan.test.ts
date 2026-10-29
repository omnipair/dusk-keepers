import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import test from "node:test";
import {
  resolveLiquidationAccounts,
  decodeLiquidationQuote,
  buildFlashPlan,
  buildEmergencyPlan,
  liquidationInstruction,
} from "../packages/dusk-adapter/src/liquidation.ts";
const layout = JSON.parse(
  readFileSync("protocol/keeper-account-layout.v1.json", "utf8"),
);
const contract = JSON.parse(
  readFileSync("protocol/keeper-instructions.v1.json", "utf8"),
);
for (const file of readdirSync("fixtures/litesvm").filter((f) =>
  f.endsWith(".json"),
)) {
  const f = JSON.parse(readFileSync(`fixtures/litesvm/${file}`, "utf8"));
  test(`flash/emergency independent native plans: ${file}`, () => {
    const context = resolveLiquidationAccounts(
      layout,
      f.snapshot,
      f.position,
      f.buyer,
      f.kind,
      f.debtAsset,
    );
    const quote = decodeLiquidationQuote(
      f.action,
      Buffer.from(f.quoteBase64, "base64"),
    );
    const plan =
      f.action === "flash"
        ? buildFlashPlan(contract, context, quote, f.maxRepayment)
        : buildEmergencyPlan(contract, context, quote);
    const native = JSON.parse(
      execFileSync("target/debug/dusk-keeper", ["--flash-plan"], {
        input: JSON.stringify({ ...f, internal: true }),
        encoding: "utf8",
      }),
    );
    assert.deepEqual(native.instructions, plan);
    assert.ok(plan.length >= 3);
    const observation = liquidationInstruction(
      contract,
      context,
      "observe_liquidation",
      { debtAsset: f.debtAsset },
    );
    const nativeObserve = JSON.parse(
      execFileSync("target/debug/dusk-keeper", ["--flash-plan"], {
        input: JSON.stringify({ ...f, action: "observe" }),
        encoding: "utf8",
      }),
    );
    assert.deepEqual(nativeObserve.instructions, [observation]);
    assert.throws(
      () =>
        decodeLiquidationQuote(
          f.action,
          Buffer.concat([
            Buffer.from(f.quoteBase64, "base64"),
            Buffer.from([0]),
          ]),
        ),
      /trailing/,
    );
    assert.throws(
      () =>
        decodeLiquidationQuote(
          f.action,
          Buffer.from(f.quoteBase64, "base64").subarray(0, 5),
        ),
      /truncated/,
    );
  });
}
