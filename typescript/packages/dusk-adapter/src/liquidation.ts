/** Atomic #45 instruction plans. This module does not sign or submit. */
import { createHash } from "node:crypto";
import { decodePubkey, findProgramAddress } from "./resolution.ts";
import { encodeKeeperInstructionJson } from "./codec.ts";
import type { InstructionContract } from "./envelope.ts";
import { decodeMintFees, decodeClockEpoch } from "./transfer-fees.ts";

export interface NativeInstruction {
  program_id: number[];
  accounts: Array<{
    pubkey: number[];
    is_signer: boolean;
    is_writable: boolean;
  }>;
  data: number[];
}
export interface BankSnapshot {
  slot: number;
  accounts: Record<
    string,
    { owner: string; data: number[]; executable: boolean } | null
  >;
}
export interface LiquidationLayout {
  programId: string;
  accounts: Array<{
    name: string;
    sizeWithDiscriminator: number;
    fields: Array<{ path: string; offset: number; size: number }>;
  }>;
}
const CLOCK = "SysvarC1ock11111111111111111111111111111111";
const ATA = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const ZERO = "11111111111111111111111111111111";
const bytes = (key: string) => decodePubkey(key, "invalid_seed_value");
function base58(data: Uint8Array): string {
  const alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  let n = 0n;
  for (const b of data) n = (n << 8n) | BigInt(b);
  let out = "";
  while (n) {
    out = alphabet[Number(n % 58n)] + out;
    n /= 58n;
  }
  for (const b of data) {
    if (b) break;
    out = "1" + out;
  }
  return out;
}
function reader(layout: LiquidationLayout, name: string, data: number[]) {
  const spec = layout.accounts.find((a) => a.name === name);
  if (!spec || data.length !== spec.sizeWithDiscriminator)
    throw new Error("account layout mismatch");
  if (
    !Buffer.from(data.slice(0, 8)).equals(
      createHash("sha256").update(`account:${name}`).digest().subarray(0, 8),
    )
  )
    throw new Error("account discriminator");
  const field = (path: string) => {
    const f = spec.fields.find((f) => f.path === path);
    if (!f) throw new Error(`missing field ${path}`);
    return Buffer.from(data.slice(f.offset, f.offset + f.size));
  };
  return {
    key: (path: string) => base58(field(path)),
    int: (path: string) => {
      const b = field(path);
      let n = 0n;
      for (let i = b.length - 1; i >= 0; i--) n = (n << 8n) | BigInt(b[i]);
      return n;
    },
  };
}
export function resolveLiquidationAccounts(
  layout: LiquidationLayout,
  snapshot: BankSnapshot,
  position: string,
  buyer: string,
  kind: "BorrowPosition" | "LeveragePosition",
  debtAsset: 0 | 1,
) {
  if (debtAsset !== 0 && debtAsset !== 1) throw new Error("invalid debt side");
  const program = layout.programId;
  const account = (key: string, owner?: string) => {
    const a = snapshot.accounts[key];
    if (!a || a.executable || (owner && a.owner !== owner))
      throw new Error(`invalid account ${key}`);
    return a;
  };
  const p = reader(layout, kind, account(position, program).data);
  const market = p.key("market");
  const m = reader(layout, "Market", account(market, program).data);
  const leverage = kind === "LeveragePosition";
  if (leverage && p.int("debt_asset") !== BigInt(debtAsset))
    throw new Error("wrong leverage side");
  if (p.key("active_liquidation_session") !== ZERO)
    throw new Error("position locked");
  const debt = debtAsset === 0 ? "base" : "quote",
    collateral = debtAsset === 0 ? "quote" : "base";
  const debtMint = m.key(`${debt}_side.asset_mint`),
    collateralMint = m.key(`${collateral}_side.asset_mint`);
  const dm = account(debtMint),
    cm = account(collateralMint);
  decodeMintFees(dm.owner, Buffer.from(dm.data));
  decodeMintFees(cm.owner, Buffer.from(cm.data));
  const clock = account(CLOCK);
  decodeClockEpoch(clock.owner, Buffer.from(clock.data), snapshot.slot);
  const derive = (seeds: Buffer[]) =>
    findProgramAddress(seeds, program).address;
  const ata = (owner: string, mint: string, token: string) =>
    findProgramAddress([bytes(owner), bytes(token), bytes(mint)], ATA).address;
  const owner = p.key("owner");
  const named: Record<string, string> = {
    market,
    buyer,
    collateral_mint: collateralMint,
    debt_mint: debtMint,
    [leverage ? "leverage_position" : "borrow_position"]: position,
    collateral_vault: leverage
      ? derive([
          Buffer.from("leverage_collateral"),
          bytes(market),
          bytes(collateralMint),
        ])
      : m.key(`${collateral}_side.collateral_vault`),
    reserve_vault: m.key(`${debt}_side.reserve_vault`),
    interest_vault: m.key(`${debt}_side.interest_vault`),
    insurance_vault: m.key(`insurance.${debt}_vault`),
    buyer_collateral_account: ata(buyer, collateralMint, cm.owner),
    buyer_refund_account: ata(buyer, debtMint, dm.owner),
    owner_debt_account: ata(owner, debtMint, dm.owner),
    debt_token_program: dm.owner,
    futarchy_authority: derive([Buffer.from("futarchy_authority")]),
    session: derive([Buffer.from("liquidation-session"), bytes(position)]),
    repayment_vault: derive([
      Buffer.from("liquidation-payment"),
      bytes(position),
      bytes(debtMint),
    ]),
    event_authority: derive([Buffer.from("__event_authority")]),
    program,
  };
  const partner = p.key(
    leverage ? "referral_partner" : `${debt}_referral_partner`,
  );
  if (partner !== ZERO) {
    named.referral_partner = partner;
    named.referral_accrual = derive([
      Buffer.from("referral_accrual"),
      bytes(partner),
      bytes(market),
      bytes(debtMint),
    ]);
  }
  const active = [
    "base_hlp_vault.hlp_supply",
    "quote_hlp_vault.hlp_supply",
    "base_hlp_vault.residual_exposure",
    "quote_hlp_vault.residual_exposure",
  ].some((f) => m.int(f) !== 0n);
  const hlp = active
    ? [
        "ylp_mint",
        "base_hlp_vault.ylp_vault",
        "quote_hlp_vault.ylp_vault",
        "base_side.interest_vault",
        "quote_side.interest_vault",
      ].map((f) => m.key(f))
    : [];
  return {
    named,
    hlp,
    collateralReserve: m.key(`${collateral}_side.reserve_vault`),
    program,
    position,
    debtAsset,
  };
}
export function liquidationInstruction(
  contract: InstructionContract,
  context: ReturnType<typeof resolveLiquidationAccounts>,
  name: string,
  args: Record<string, unknown>,
  internal = false,
): NativeInstruction {
  const specificationKey = `dusk:${name}`;
  const spec = contract.instructions.find((i) => i.key === specificationKey);
  if (!spec || spec.programId !== context.program)
    throw new Error("instruction provenance");
  const named = {
    ...context.named,
    ...(internal
      ? { collateral_reserve_vault: context.collateralReserve }
      : {}),
  };
  const accounts = spec.accounts.map((a) => {
    const found = named[a.name] ?? a.fixedAddress;
    if (!found && !a.optional) throw new Error(`missing ${a.name}`);
    return {
      pubkey: Array.from(bytes(found ?? context.program)),
      is_signer: a.signer && !!found,
      is_writable: a.writable && !!found,
    };
  });
  if (internal)
    accounts.push(
      ...context.hlp.map((k) => ({
        pubkey: Array.from(bytes(k)),
        is_signer: false,
        is_writable: true,
      })),
    );
  return {
    program_id: Array.from(bytes(context.program)),
    accounts,
    data: Array.from(
      encodeKeeperInstructionJson(contract, specificationKey, args),
    ),
  };
}
export function liquidationResources(): NativeInstruction[] {
  return [
    [1, 262144],
    [2, 1400000],
  ].map(([op, value]) => {
    const data = Buffer.alloc(5);
    data[0] = op;
    data.writeUInt32LE(value, 1);
    return {
      program_id: Array.from(
        bytes("ComputeBudget111111111111111111111111111111"),
      ),
      accounts: [],
      data: Array.from(data),
    };
  });
}

/** Exact quote return layouts; amounts are decimal strings for JSON parity. */
export function decodeLiquidationQuote(
  kind: "flash" | "emergency",
  data: Uint8Array,
): Record<string, any> {
  const b = Buffer.from(data);
  let offset = 0;
  const integer = (width: number) => {
    if (offset + width > b.length) throw new Error("truncated return data");
    let n = 0n;
    for (let i = width - 1; i >= 0; i--) n = (n << 8n) | BigInt(b[offset + i]);
    offset += width;
    return n;
  };
  const u64 = () => integer(8).toString(),
    u128 = () => integer(16).toString(),
    u16 = () => Number(integer(2));
  const bool = () => {
    const n = integer(1);
    if (n > 1n) throw new Error("invalid bool");
    return n === 1n;
  };
  const health = () => ({
    collateralValue: u64(),
    debt: u64(),
    maintenanceBps: u16(),
  });
  const fee = () => ({
    total: u64(),
    protocol: u64(),
    insurance: u64(),
    lp: u64(),
  });
  const rates = () => ({ buyerDiscountBps: u16(), emergencyRewardBps: u16() });
  let q: Record<string, any>;
  if (kind === "flash")
    q = {
      full: bool(),
      collateralDebit: u64(),
      collateralCredit: u64(),
      payment: u64(),
      repayment: u64(),
      ownerSurplus: u64(),
      fee: fee(),
      debtSharesToBurn: u128(),
      healthBefore: health(),
      healthAfter: health(),
      rates: rates(),
    };
  else
    q = {
      referencePriceNad: u64(),
      referenceDepthNad: u128(),
      full: bool(),
      collateralDebit: u64(),
      collateralCredit: u64(),
      swapOutput: u64(),
      repayment: u64(),
      sharesToBurn: u128(),
      rewardCredit: u64(),
      rewardDebit: u64(),
      fee: fee(),
      insuranceFeeDebit: u64(),
      revenueFeeDebit: u64(),
      ownerDebit: u64(),
      ownerCredit: u64(),
      insuranceDebit: u64(),
      loss: {
        principalRepaid: u64(),
        interestPaid: u64(),
        insuranceCredit: u64(),
        principalWrittenOff: u64(),
        interestCanceled: u64(),
        surplus: u64(),
      },
      healthBefore: health(),
      healthAfter: health(),
      rates: rates(),
    };
  if (offset !== b.length) throw new Error("trailing return data");
  return q;
}
export function buildFlashPlan(
  contract: InstructionContract,
  context: ReturnType<typeof resolveLiquidationAccounts>,
  quote: ReturnType<typeof decodeLiquidationQuote>,
  maxRepayment: string,
  options: {
    prefix?: NativeInstruction[];
    route?: NativeInstruction[];
    internal?: boolean;
  } = {},
): NativeInstruction[] {
  const prefix = options.prefix ?? liquidationResources(),
    route = options.route ?? [],
    internal = options.internal ?? true;
  if (internal && route.length)
    throw new Error("internal settlement cannot include a route");
  const settleIndex = prefix.length + 1 + route.length;
  if (settleIndex > 65535) throw new Error("settle index overflow");
  const begin = liquidationInstruction(
    contract,
    context,
    "begin_flash_liquidation",
    {
      position: context.position,
      debtAsset: context.debtAsset,
      maxRepayment,
      maxPayment: quote.payment,
      minCollateralCredit: quote.collateralCredit,
      full: quote.full,
      settleIndex,
    },
  );
  return [
    ...prefix,
    begin,
    ...route,
    liquidationInstruction(
      contract,
      context,
      "settle_flash_liquidation",
      {},
      internal,
    ),
  ];
}
export function buildEmergencyPlan(
  contract: InstructionContract,
  context: ReturnType<typeof resolveLiquidationAccounts>,
  quote: ReturnType<typeof decodeLiquidationQuote>,
): NativeInstruction[] {
  return [
    ...liquidationResources(),
    liquidationInstruction(
      contract,
      context,
      "emergency_liquidation",
      {
        debtAsset: context.debtAsset,
        collateralDebit: quote.collateralDebit,
        full: quote.full,
        minRewardCredit: quote.rewardCredit,
        minSwapOutput: quote.swapOutput,
      },
      true,
    ),
  ];
}
