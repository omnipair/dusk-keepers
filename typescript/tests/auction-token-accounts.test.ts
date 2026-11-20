import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import {
  decodePubkey,
  findProgramAddress,
} from "../packages/dusk-adapter/src/resolution.ts";
const vectors = JSON.parse(
  readFileSync(
    new URL(
      "../../fixtures/conformance/v1/auction-token-accounts.json",
      import.meta.url,
    ),
    "utf8",
  ),
);
for (const c of vectors.cases)
  test(`native auction ATAs match SPL vectors: ${c.soldProgram}/${c.acceptedProgram}`, () => {
    const ata = (owner: string, mint: string, program: string) =>
      findProgramAddress(
        [owner, program, mint].map((k) =>
          decodePubkey(k, "invalid_seed_value"),
        ),
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
      ).address;
    assert.deepEqual(
      {
        bidder_payment_account: ata(
          vectors.bidder,
          vectors.acceptedMint,
          c.acceptedProgram,
        ),
        bidder_receive_account: ata(
          vectors.bidder,
          vectors.soldMint,
          c.soldProgram,
        ),
        treasury_payment_account: ata(
          vectors.treasury,
          vectors.acceptedMint,
          c.acceptedProgram,
        ),
        staking_vault_payment_account: ata(
          vectors.stakingVault,
          vectors.acceptedMint,
          c.acceptedProgram,
        ),
      },
      c.expected,
    );
  });
