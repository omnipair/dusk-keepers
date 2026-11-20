import { readFileSync } from "node:fs";
import anchor from "@coral-xyz/anchor";
import { IdlCoder } from "@coral-xyz/anchor/dist/cjs/coder/borsh/idl.js";
import { PublicKey } from "@solana/web3.js";
import {
  AccountLayout,
  getAssociatedTokenAddressSync,
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
} from "@solana/spl-token";
import { bank } from "./liquidation-bank.ts";
const root = process.cwd();
const dusk = JSON.parse(readFileSync("artifacts/dusk.json", "utf8"));
const program = new PublicKey(dusk.address);
const bn = (n: any) => new anchor.BN(n.toString());
const codec = (name: string) =>
  IdlCoder.typeDefLayout({
    typeDef: dusk.types.find((t: any) => t.name === name),
    types: dusk.types,
  });
const decode = (_idl: any, name: string, raw: Uint8Array) =>
  codec(name).decode(Buffer.from(raw).subarray(8));
const data = (_idl: any, name: string, value: any) => {
  const raw = Buffer.alloc(32768),
    size = codec(name).encode(value, raw);
  return Buffer.concat([
    Buffer.from(dusk.accounts.find((a: any) => a.name === name).discriminator),
    raw.subarray(0, size),
  ]);
};
const put = (svm: any, address: PublicKey, owner: PublicKey, data: Buffer) =>
  svm.setAccount(address, {
    owner,
    data,
    lamports: 10000000n,
    executable: false,
  });
export function auctionFixture(
  soldProgram: PublicKey,
  paymentProgram: PublicKey,
  bidder: PublicKey,
  transferFeeBps = 0,
) {
  const captured = JSON.parse(
      readFileSync(root + "/fixtures/litesvm/07-flash-LeveragePosition.json"),
    ),
    svm = bank(captured),
    known = new Set(Object.keys(captured.bank));
  const find = (name) => {
    const discriminator = Buffer.from(
      dusk.accounts.find((a) => a.name === name).discriminator,
    );
    return new PublicKey(
      Object.keys(captured.bank).find((k) =>
        Buffer.from(captured.bank[k].data).subarray(0, 8).equals(discriminator),
      ),
    );
  };
  const marketKey = find("Market"),
    futarchyKey = find("FutarchyAuthority"),
    market = decode(dusk, "Market", svm.getAccount(marketKey).data),
    futarchy = decode(
      dusk,
      "FutarchyAuthority",
      svm.getAccount(futarchyKey).data,
    );
  const signer = { publicKey: bidder };
  const acceptedMint = new PublicKey(Buffer.alloc(32, 123)),
    mintData = Buffer.alloc(82);
  mintData[44] = 6;
  mintData[45] = 1;
  put(svm, acceptedMint, paymentProgram, mintData);
  known.add(acceptedMint.toBase58());
  const assets = [market.base_side.asset_mint, market.quote_side.asset_mint],
    programs = [soldProgram, soldProgram];
  for (let i = 0; i < 2; i++) {
    const a = svm.getAccount(assets[i]);
    let data = Buffer.from(a.data);
    if (transferFeeBps > 0) {
      if (!programs[i].equals(TOKEN_2022_PROGRAM_ID))
        throw new Error("transfer fees require Token-2022");
      const extended = Buffer.alloc(278);
      data.copy(extended);
      extended[165] = 1;
      extended.writeUInt16LE(1, 166);
      extended.writeUInt16LE(108, 168);
      for (const offset of [242, 260]) {
        extended.writeBigUInt64LE(0n, offset);
        extended.writeBigUInt64LE(1000000n, offset + 8);
        extended.writeUInt16LE(transferFeeBps, offset + 16);
      }
      data = extended;
    }
    svm.setAccount(assets[i], { ...a, data, owner: programs[i] });
  }
  for (const address of known) {
    const a = svm.getAccount(new PublicKey(address));
    if (
      !a ||
      a.data.length < 165 ||
      ![TOKEN_PROGRAM_ID.toBase58(), TOKEN_2022_PROGRAM_ID.toBase58()].includes(
        a.owner.toBase58(),
      )
    )
      continue;
    const mint = new PublicKey(Buffer.from(a.data).subarray(0, 32));
    const index = assets.findIndex((m) => m.equals(mint));
    if (index >= 0) {
      let data = Buffer.from(a.data);
      if (transferFeeBps > 0) {
        const extended = Buffer.alloc(178);
        data.copy(extended);
        extended[165] = 2;
        extended.writeUInt16LE(2, 166);
        extended.writeUInt16LE(8, 168);
        data = extended;
      }
      svm.setAccount(new PublicKey(address), {
        ...a,
        data,
        owner: programs[index],
      });
    }
  }
  for (const side of [market.base_side, market.quote_side]) {
    for (const name of [
      "fee_swap_auction",
      "fee_interest_auction",
      "buyback_swap_auction",
      "buyback_interest_auction",
    ])
      side.fees[name] = {
        accepted_mint: acceptedMint,
        virtual_sold_reserve: bn(1000000),
        virtual_payment_reserve: bn(1000000),
        sold_rate_per_day: bn(0),
        last_update_slot: bn(svm.getClock().slot),
        stream_remainder: bn(0),
      };
    for (const name of [
      "swap_protocol_fee_liability",
      "swap_buyback_fee_liability",
      "interest_protocol_fee_liability",
      "interest_buyback_fee_liability",
    ])
      side.fees[name] = side.fees[name].addn(1000);
    side.fees.swap_fee_custody_balance =
      side.fees.swap_fee_custody_balance.addn(2000);
    const vault = svm.getAccount(side.reserve_vault),
      raw = Buffer.from(vault.data);
    raw.writeBigUInt64LE(raw.readBigUInt64LE(64) + 4000n, 64);
    svm.setAccount(side.reserve_vault, { ...vault, data: raw });
  }
  put(svm, marketKey, program, data(dusk, "Market", market));
  futarchy.fee_auction.accepted_mint = acceptedMint;
  futarchy.buyback_auction.accepted_mint = acceptedMint;
  put(svm, futarchyKey, program, data(dusk, "FutarchyAuthority", futarchy));
  const token = (owner, mint, tokenProgram, amount) => {
    const address = getAssociatedTokenAddressSync(
        mint,
        owner,
        true,
        tokenProgram,
      ),
      raw = Buffer.alloc(
        transferFeeBps > 0 && assets.some((asset) => asset.equals(mint))
          ? 178
          : 165,
      );
    if (raw.length > 165) {
      raw[165] = 2;
      raw.writeUInt16LE(2, 166);
      raw.writeUInt16LE(8, 168);
    }
    AccountLayout.encode(
      {
        mint,
        owner,
        amount,
        delegateOption: 0,
        delegate: PublicKey.default,
        state: 1,
        isNativeOption: 0,
        isNative: 0n,
        delegatedAmount: 0n,
        closeAuthorityOption: 0,
        closeAuthority: PublicKey.default,
      },
      raw,
    );
    put(svm, address, tokenProgram, raw);
    known.add(address.toBase58());
    return address;
  };
  for (let i = 0; i < 2; i++)
    token(signer.publicKey, assets[i], programs[i], 1000000n);
  token(signer.publicKey, acceptedMint, paymentProgram, 1000000n);
  for (const config of [futarchy.fee_auction, futarchy.buyback_auction])
    for (const recipient of [
      config.recipients.treasury,
      config.recipients.staking_vault,
    ])
      token(recipient, acceptedMint, paymentProgram, 0n);
  put(svm, signer.publicKey, PublicKey.default, Buffer.alloc(0));
  known.add(signer.publicKey.toBase58());
  return {
    svm,
    known,
    marketKey,
    futarchyKey,
    acceptedMint,
    assets,
    signer: bidder,
    transferFeeBps,
  };
}
