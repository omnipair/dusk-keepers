import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { Keypair, PublicKey } from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID } from "@solana/spl-token";
import { auctionFixture } from "./helpers/auction-bank.ts";
import { run } from "./helpers/lifecycle-bank.ts";
import { encodeKeeperInstructionJson } from "../packages/dusk-adapter/src/codec.ts";
const contract = JSON.parse(
  readFileSync("protocol/keeper-instructions.v1.json", "utf8"),
);
const cases = [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID]
  .flatMap((sold) =>
    [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID].map((payment) => ({
      sold,
      payment,
      transferFeeBps: 0,
    })),
  )
  .concat(
    [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID].map((payment) => ({
      sold: TOKEN_2022_PROGRAM_ID,
      payment,
      transferFeeBps: 100,
    })),
  );
for (const { sold, payment, transferFeeBps } of cases)
  test(`native revenue worker simulates eight auctions for ${sold}/${payment}, fee ${transferFeeBps}`, async () => {
    const signer = Keypair.generate(),
      f = auctionFixture(sold, payment, signer.publicKey, transferFeeBps);
    const checked = await run(f as any, signer.publicKey, undefined, {
      KEEPER_PROFILE: "auction-arbitrage",
      KEEPER_AUCTION_MAX_SOLD: "1000",
      KEEPER_AUCTION_MAX_PAYMENT: "1000000",
      KEEPER_AUCTION_SLIPPAGE_BPS: "0",
    });
    assert.equal(checked.simulations.length, 8, checked.output);
    for (const simulated of checked.simulations) {
      assert.equal(simulated.error, null);
      const tx = simulated.transaction;
      const instruction = tx.message.compiledInstructions[0],
        raw = Buffer.from(instruction.data);
      const native = encodeKeeperInstructionJson(
        contract,
        "dusk:settle_protocol_auction",
        {
          lane: raw[8] === 0 ? "fee" : "buyback",
          source: raw[9] === 0 ? "swap" : "interest",
          soldAmount: "1000",
          maxPaymentAmount: "18446744073709551615",
        },
      );
      assert.equal(
        Buffer.from(native).toString("hex"),
        raw.toString("hex"),
        "independent TypeScript encoding matches the Rust worker intent",
      );
    }
    assert.match(checked.output, /keeper_once evaluated=8/);
    assert.ok(!checked.requests.some((r) => r.method === "sendTransaction"));
  });
