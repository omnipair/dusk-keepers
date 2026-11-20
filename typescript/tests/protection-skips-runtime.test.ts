import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { PublicKey } from "@solana/web3.js";
import anchor from "@coral-xyz/anchor";
import { IdlCoder } from "@coral-xyz/anchor/dist/cjs/coder/borsh/idl.js";
import { bank } from "./helpers/liquidation-bank.ts";
import { run } from "./helpers/lifecycle-bank.ts";
const idl = JSON.parse(
  readFileSync("artifacts/leverage_delegate.json", "utf8"),
);
const layout = IdlCoder.typeDefLayout({
  typeDef: idl.types.find((t: any) => t.name === "LeverageOrder"),
  types: idl.types,
});
const delegateProgram = new PublicKey(idl.address);
const orderSimulations = (checked: Awaited<ReturnType<typeof run>>) =>
  checked.simulations.filter((s) =>
    s.transaction.message.staticAccountKeys.some((k) =>
      k.equals(delegateProgram),
    ),
  );
for (const kind of ["tp", "sl"])
  test(`native protection worker skips unmet ${kind} without submitting`, async () => {
    const captured = JSON.parse(
      readFileSync(`fixtures/orders-signed/${kind}-10000.json`, "utf8"),
    );
    const svm = bank(captured),
      order = new PublicKey(captured.order),
      a = svm.getAccount(order)!,
      raw = Buffer.from(a.data),
      value = layout.decode(raw.subarray(8));
    value.trigger_price_nad = new anchor.BN(
      kind === "tp" ? "18446744073709551615" : "1",
    );
    layout.encode(value, raw.subarray(8));
    svm.setAccount(order, { ...a, data: raw });
    const checked = await run(
      { svm, known: new Set(Object.keys(captured.bank)) } as any,
      new PublicKey(captured.buyer),
      undefined,
      { KEEPER_PROFILE: "leverage", KEEPER_ORDER_MIN_BOUNTY_CREDIT: "0" },
    );
    assert.doesNotMatch(
      checked.output,
      /native trigger, settlement and bounty checks passed/,
    );
    const attempts = orderSimulations(checked);
    assert.equal(attempts.length, 1, checked.output);
    const trigger = idl.errors.find(
      (e: any) => e.name === "TriggerNotMet",
    ).code;
    assert.match(attempts[0].error ?? "", new RegExp(`code: *${trigger}([^0-9]|$)`));
    assert.ok(!checked.requests.some((r) => r.method === "sendTransaction"));
    assert.ok(
      (svm.getAccount(order)?.lamports ?? 0n) > 0n,
      "unmet order remains available",
    );
  });
test("native protection worker skips a position already closed by liquidation", async () => {
  const captured = JSON.parse(
    readFileSync("fixtures/orders-signed/tp-10000.json", "utf8"),
  );
  const svm = bank(captured);
  svm.setAccount(new PublicKey(captured.position), {
    owner: PublicKey.default,
    data: Buffer.alloc(0),
    lamports: 0n,
    executable: false,
  });
  const checked = await run(
    { svm, known: new Set(Object.keys(captured.bank)) } as any,
    new PublicKey(captured.buyer),
    undefined,
    { KEEPER_PROFILE: "leverage", KEEPER_ORDER_MIN_BOUNTY_CREDIT: "0" },
  );
  assert.match(checked.output, /order_skipped/);
  assert.equal(orderSimulations(checked).length, 0);
  assert.ok(!checked.requests.some((r) => r.method === "sendTransaction"));
});
