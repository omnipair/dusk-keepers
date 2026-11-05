// Synthetic oracle policy layered onto a captured program-generated bank.
import { readFileSync } from "node:fs";
import anchor from "@coral-xyz/anchor";
import { PublicKey } from "@solana/web3.js";
import { resolveLiquidationAccounts } from "../../packages/dusk-adapter/src/liquidation.ts";
const { BN, BorshAccountsCoder } = anchor;
const read = (path: string) => JSON.parse(readFileSync(path, "utf8"));
const layout = read("protocol/keeper-account-layout.v1.json");
const coder = new BorshAccountsCoder(read("artifacts/dusk.json"));
export const base = new PublicKey(Buffer.alloc(32, 71));
export const quote = new PublicKey(Buffer.alloc(32, 72));
export const provider = new PublicKey(Buffer.alloc(32, 73));

export function withOracle(f: any, format = 1, weight = 2500) {
  const context = resolveLiquidationAccounts(layout, f.snapshot, f.position, f.buyer, f.kind, f.debtAsset);
  const market = context.named.market;
  const state: any = coder.decode("Market", Buffer.from(f.bank[market].data));
  state.oracle.policy = {
    format, provider, base_account: base, quote_account: [2, 4].includes(format) ? quote : PublicKey.default,
    base_feed_id: Array(32).fill(7), quote_feed_id: Array(32).fill([2, 4].includes(format) ? 8 : 0),
    lambda_bps: weight, adjustment_cap_nad: new BN(2_000_000_000),
    max_age_seconds: 60, max_age_slots: new BN(100), max_confidence_bps: 100, max_publish_skew_seconds: 0,
  };
  // Anchor 0.31's encode() allocates only 1000 bytes; this market is larger.
  // Use its IDL-generated layout with the fixture's full account allocation.
  const encoded = Buffer.alloc(f.bank[market].data.length);
  encoded.set(coder.accountDiscriminator("Market"));
  (coder as any).accountLayouts.get("Market").layout.encode(state, encoded.subarray(8));
  const data = Array.from(encoded);
  f.bank[market].data = data;
  f.snapshot.accounts[market].data = data;
  return { f, state, market };
}
