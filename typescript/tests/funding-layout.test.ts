import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { BorshAccountsCoder } from "@coral-xyz/anchor";

test("layout 17 signed translations leave the native keeper offsets aligned", () => {
  const idl = JSON.parse(readFileSync("artifacts/dusk.json", "utf8"));
  const bytes = readFileSync("fixtures/conformance/v1/market-layout17.bin");
  const decoded = new BorshAccountsCoder(idl).decode("Market", bytes);
  assert.equal(decoded.version, 17);
  assert.equal(decoded.amm.hlp_funding_base_pricing_offset_nad.toString(), "-1267650600228229401496703205393");
  assert.equal(decoded.amm.hlp_funding_quote_pricing_offset_nad.toString(), "79228162514264337593543950355");
  const layout = JSON.parse(readFileSync("protocol/keeper-account-layout.v1.json", "utf8"));
  const market = layout.accounts.find((row: any) => row.name === "Market");
  assert.equal(bytes.length, market.sizeWithDiscriminator);
  for (const [path, expected] of [
    ["base_side.reserves.live_reserve", decoded.base_side.reserves.live_reserve],
    ["quote_side.reserves.live_reserve", decoded.quote_side.reserves.live_reserve],
    ["operator.epoch", "987654321"],
    ["last_update_slot", "123456789012345"],
  ] as const) {
    const field = market.fields.find((row: any) => row.path === path);
    assert.equal(bytes.readBigUInt64LE(field.offset).toString(), String(expected), path);
  }
});
