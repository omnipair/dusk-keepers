import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { feePhase, feeCredit, decodeMintFees, decodeClockEpoch, TOKEN_2022_PROGRAM, TOKEN_PROGRAM } from "../packages/dusk-adapter/src/transfer-fees.ts";
test("native fee calculations match the shared Dusk SDK vectors", () => {
  const cases = JSON.parse(readFileSync(new URL("../../fixtures/conformance/v1/transfer-fee-notice.json", import.meta.url), "utf8"));
  for (const v of cases) {
    const fee = (f: [string, number, string]) => ({ epoch: BigInt(f[0]), bps: f[1], maximum: BigInt(f[2]) });
    const schedule = { older: fee(v.older), newer: fee(v.newer) };
    assert.equal(feePhase(schedule, BigInt(v.epoch)), v.phase);
    for (const mode of ["effective", "admission", "liquidation"] as const) assert.equal(feeCredit(schedule, BigInt(v.epoch), BigInt(v.gross), mode).toString(), v[mode], v.name);
  }
});
test("mint and Clock readers reject malformed identity and epoch provenance", () => {
  const data = Buffer.alloc(278); data[45] = 1; data[165] = 1; data.writeUInt16LE(1,166); data.writeUInt16LE(108,168);
  data.writeBigUInt64LE(12n,260); data.writeUInt16LE(10000,276);
  assert.equal(feePhase(decodeMintFees(TOKEN_2022_PROGRAM,data),11n),"finalNotice");
  assert.throws(() => decodeMintFees(TOKEN_PROGRAM,data));
  assert.throws(() => decodeMintFees("fake",data));
  assert.throws(() => decodeMintFees(TOKEN_2022_PROGRAM,data.subarray(0,270)));
  const clock = Buffer.alloc(40); clock.writeBigUInt64LE(123n); clock.writeBigUInt64LE(11n,16);
  assert.equal(decodeClockEpoch("Sysvar1111111111111111111111111111111111111",clock,123),11n);
  assert.throws(() => decodeClockEpoch("Sysvar1111111111111111111111111111111111111",clock,124));
});
