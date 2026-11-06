import { nativePlan } from "./helpers/native-planner.ts";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
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
    const native = nativePlan({ ...f, internal: true });
    assert.deepEqual(native.instructions, plan);
    assert.ok(plan.length >= 3);
    const healthPreview = liquidationInstruction(
      contract,
      context,
      "preview_liquidation_health",
      { debtAsset: f.debtAsset },
    );
    const nativeHealth = nativePlan({ ...f, action: "health" });
    assert.deepEqual(nativeHealth.instructions, [healthPreview]);
    assert.ok(healthPreview.accounts.every(meta => !meta.is_writable && !meta.is_signer));
    assert.ok(!plan.some(ix => Buffer.from(ix.data).equals(Buffer.from(healthPreview.data))));
    assert.throws(() => liquidationInstruction(contract, context, "observe_liquidation", { debtAsset: f.debtAsset }));
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
