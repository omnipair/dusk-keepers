import { decodePubkey } from "./resolution.ts";
import { createHash } from "node:crypto";

import type { InstructionContract } from "./envelope.ts";

export interface DelegatedCpiArgs {
  readonly beforeIxData: Uint8Array;
  readonly afterIxData: Uint8Array;
  readonly beforeAccountsLen: number;
}

export interface DelegatedCloseLeverageArgs {
  readonly debtAsset: number;
  readonly minAmountOut: bigint;
  /** What fraction of the position to close. */
  readonly closeBps: number;
  readonly delegated: DelegatedCpiArgs;
}

export interface ExecuteOrderArgs {
  readonly orderId: bigint;
}

export type ProtocolAuctionLane = "fee" | "buyback";
export type ProtocolRevenueSource = "swap" | "interest";

export interface SettleProtocolAuctionArgs {
  readonly lane: ProtocolAuctionLane;
  readonly source: ProtocolRevenueSource;
  readonly soldAmount: bigint;
  readonly maxPaymentAmount: bigint;
}

export type KeeperInstructionArguments =
  | {
      readonly specificationKey: "dusk:observe_liquidation";
      readonly arguments: { readonly debtAsset: number };
    }
  | {
      readonly specificationKey: "dusk:preview_flash_liquidation";
      readonly arguments: {
        readonly debtAsset: number;
        readonly maxRepayment: bigint;
        readonly full: boolean;
      };
    }
  | {
      readonly specificationKey: "dusk:preview_emergency_liquidation";
      readonly arguments: {
        readonly debtAsset: number;
        readonly collateralDebit: bigint;
        readonly full: boolean;
      };
    }
  | {
      readonly specificationKey: "dusk:begin_flash_liquidation";
      readonly arguments: {
        readonly position: Uint8Array;
        readonly debtAsset: number;
        readonly maxRepayment: bigint;
        readonly maxPayment: bigint;
        readonly minCollateralCredit: bigint;
        readonly minBuyerCredit: bigint;
        readonly full: boolean;
        readonly settleIndex: number;
      };
    }
  | {
      readonly specificationKey: "dusk:emergency_liquidation";
      readonly arguments: {
        readonly debtAsset: number;
        readonly collateralDebit: bigint;
        readonly full: boolean;
        readonly minRewardCredit: bigint;
        readonly minSwapOutput: bigint;
      };
    }
  | { readonly specificationKey: "dusk:settle_flash_liquidation" }
  | {
      readonly specificationKey: "dusk:preview_hlp_flash";
      readonly arguments: { readonly targetAsset: number };
    }
  | {
      readonly specificationKey: "dusk:begin_hlp_flash";
      readonly arguments: {
        readonly targetAsset: number;
        readonly maxPayment: bigint;
        readonly allowFull: boolean;
        readonly maxInsuranceDraw: bigint;
        readonly maxSocializedLoss: bigint;
        readonly maxCollateralDebit: bigint;
        readonly minCollateralCredit: bigint;
        readonly settleIndex: number;
      };
    }
  | { readonly specificationKey: "dusk:settle_hlp_flash" }
  | {
      readonly specificationKey: "dusk:close_insolvent_hlp";
      readonly arguments: {
        readonly targetAsset: number;
        readonly maxInsuranceDraw: bigint;
        readonly maxSocializedLoss: bigint;
        readonly minCallerBountyOut: bigint;
      };
    }
  | {
      readonly specificationKey: "dusk:delegated_close_leverage";
      readonly arguments: DelegatedCloseLeverageArgs;
    }
  | {
      readonly specificationKey: "leverage_delegate:before_take_profit";
      readonly arguments: ExecuteOrderArgs;
    }
  | {
      readonly specificationKey: "leverage_delegate:after_close_order";
      readonly arguments: ExecuteOrderArgs;
    }
  | {
      readonly specificationKey: "leverage_delegate:before_stop_loss";
      readonly arguments: ExecuteOrderArgs;
    }
  | {
      readonly specificationKey: "dusk:settle_protocol_auction";
      readonly arguments: SettleProtocolAuctionArgs;
    }
  | { readonly specificationKey: "dusk:queue_parameter_proposal" }
  | { readonly specificationKey: "dusk:execute_parameter_proposal" };

export type InstructionEncodingErrorCode =
  | "unsupported_instruction"
  | "invalid_arguments"
  | "contract_mismatch";

export class InstructionEncodingError extends Error {
  readonly code: InstructionEncodingErrorCode;

  constructor(code: InstructionEncodingErrorCode, message: string) {
    super(message);
    this.name = "InstructionEncodingError";
    this.code = code;
  }
}

export function encodeKeeperInstruction(
  contract: InstructionContract,
  request: KeeperInstructionArguments,
): Uint8Array {
  const specification = contract.instructions.find(
    (entry) => entry.key === request.specificationKey,
  );
  if (!specification) {
    throw encodingError(
      "contract_mismatch",
      `${request.specificationKey}: instruction is absent from the pinned contract`,
    );
  }
  const [expectedProgram, expectedName, unexpectedPart] =
    request.specificationKey.split(":");
  if (
    unexpectedPart !== undefined ||
    specification.program !== expectedProgram ||
    specification.instructionName !== expectedName
  ) {
    throw encodingError(
      "contract_mismatch",
      `${request.specificationKey}: contract instruction identity differs`,
    );
  }
  const discriminator = exactHex(
    specification.discriminatorHex,
    8,
    "contract_mismatch",
  );
  const anchorDiscriminator = createHash("sha256")
    .update(`global:${specification.instructionName}`)
    .digest()
    .subarray(0, 8);
  if (!discriminator.equals(anchorDiscriminator)) {
    throw encodingError(
      "contract_mismatch",
      `${request.specificationKey}: discriminator differs from Anchor global hash`,
    );
  }

  const writer = new BorshWriter();
  switch (request.specificationKey) {
    case "dusk:observe_liquidation":
      writer.u8(request.arguments.debtAsset);
      break;
    case "dusk:preview_flash_liquidation":
      writer.u8(request.arguments.debtAsset);
      writer.u64(request.arguments.maxRepayment);
      writer.u8(request.arguments.full ? 1 : 0);
      break;
    case "dusk:preview_emergency_liquidation":
      writer.u8(request.arguments.debtAsset);
      writer.u64(request.arguments.collateralDebit);
      writer.u8(request.arguments.full ? 1 : 0);
      break;
    case "dusk:begin_flash_liquidation":
      writer.raw(request.arguments.position);
      writer.u8(request.arguments.debtAsset);
      writer.u64(request.arguments.maxRepayment);
      writer.u64(request.arguments.maxPayment);
      writer.u64(request.arguments.minCollateralCredit);
      writer.u64(request.arguments.minBuyerCredit);
      writer.u8(request.arguments.full ? 1 : 0);
      writer.u16(request.arguments.settleIndex);
      break;
    case "dusk:emergency_liquidation":
      writer.u8(request.arguments.debtAsset);
      writer.u64(request.arguments.collateralDebit);
      writer.u8(request.arguments.full ? 1 : 0);
      writer.u64(request.arguments.minRewardCredit);
      writer.u64(request.arguments.minSwapOutput);
      break;
    case "dusk:settle_flash_liquidation":
      break;
    case "dusk:preview_hlp_flash":
      writer.u8(request.arguments.targetAsset);
      break;
    case "dusk:begin_hlp_flash":
      writer.u8(request.arguments.targetAsset);
      writer.u64(request.arguments.maxPayment);
      writer.u8(request.arguments.allowFull ? 1 : 0);
      writer.u64(request.arguments.maxInsuranceDraw);
      writer.u64(request.arguments.maxSocializedLoss);
      writer.u64(request.arguments.maxCollateralDebit);
      writer.u64(request.arguments.minCollateralCredit);
      writer.u16(request.arguments.settleIndex);
      break;
    case "dusk:settle_hlp_flash":
      break;
    case "dusk:close_insolvent_hlp":
      writer.u8(request.arguments.targetAsset);
      writer.u64(request.arguments.maxInsuranceDraw);
      writer.u64(request.arguments.maxSocializedLoss);
      writer.u64(request.arguments.minCallerBountyOut);
      break;

    case "dusk:queue_parameter_proposal":
    case "dusk:execute_parameter_proposal":
      break;
    case "dusk:delegated_close_leverage":
      writer.u8(request.arguments.debtAsset);
      writer.u64(request.arguments.minAmountOut);
      writer.u16(request.arguments.closeBps);
      writer.bytes(request.arguments.delegated.beforeIxData);
      writer.bytes(request.arguments.delegated.afterIxData);
      writer.u16(request.arguments.delegated.beforeAccountsLen);
      break;
    case "leverage_delegate:before_take_profit":
    case "leverage_delegate:after_close_order":
    case "leverage_delegate:before_stop_loss":
      writer.u64(request.arguments.orderId);
      break;
    case "dusk:settle_protocol_auction":
      writer.u8(
        enumOrdinal(request.arguments.lane, ["fee", "buyback"], "lane"),
      );
      writer.u8(
        enumOrdinal(request.arguments.source, ["swap", "interest"], "source"),
      );
      writer.u64(request.arguments.soldAmount);
      writer.u64(request.arguments.maxPaymentAmount);
      break;
  }
  return Buffer.concat([discriminator, writer.finish()]);
}

export function parseKeeperInstructionArguments(
  specificationKey: string,
  value: unknown,
): KeeperInstructionArguments {
  const argumentsValue = requireRecord(value);
  switch (specificationKey) {
    case "dusk:observe_liquidation":
      requireKeys(argumentsValue, ["debtAsset"]);
      return {
        specificationKey,
        arguments: {
          debtAsset: unsignedNumber(
            argumentsValue.debtAsset,
            0xff,
            "debt_asset",
          ),
        },
      };
    case "dusk:preview_flash_liquidation":
      requireKeys(argumentsValue, ["debtAsset", "maxRepayment", "full"]);
      return {
        specificationKey,
        arguments: {
          debtAsset: unsignedNumber(
            argumentsValue.debtAsset,
            0xff,
            "debt_asset",
          ),
          maxRepayment: decimalU64(argumentsValue.maxRepayment),
          full: booleanValue(argumentsValue.full),
        },
      };
    case "dusk:preview_emergency_liquidation":
      requireKeys(argumentsValue, ["debtAsset", "collateralDebit", "full"]);
      return {
        specificationKey,
        arguments: {
          debtAsset: unsignedNumber(
            argumentsValue.debtAsset,
            0xff,
            "debt_asset",
          ),
          collateralDebit: decimalU64(argumentsValue.collateralDebit),
          full: booleanValue(argumentsValue.full),
        },
      };
    case "dusk:begin_flash_liquidation":
      requireKeys(argumentsValue, [
        "position",
        "debtAsset",
        "maxRepayment",
        "maxPayment",
        "minCollateralCredit",
        "minBuyerCredit",
        "full",
        "settleIndex",
      ]);
      return {
        specificationKey,
        arguments: {
          position: pubkeyValue(argumentsValue.position),
          debtAsset: unsignedNumber(
            argumentsValue.debtAsset,
            0xff,
            "debt_asset",
          ),
          maxRepayment: decimalU64(argumentsValue.maxRepayment),
          maxPayment: decimalU64(argumentsValue.maxPayment),
          minCollateralCredit: decimalU64(argumentsValue.minCollateralCredit),
          minBuyerCredit: decimalU64(argumentsValue.minBuyerCredit),
          full: booleanValue(argumentsValue.full),
          settleIndex: unsignedNumber(
            argumentsValue.settleIndex,
            0xffff,
            "settle_index",
          ),
        },
      };
    case "dusk:emergency_liquidation":
      requireKeys(argumentsValue, [
        "debtAsset",
        "collateralDebit",
        "full",
        "minRewardCredit",
        "minSwapOutput",
      ]);
      return {
        specificationKey,
        arguments: {
          debtAsset: unsignedNumber(
            argumentsValue.debtAsset,
            0xff,
            "debt_asset",
          ),
          collateralDebit: decimalU64(argumentsValue.collateralDebit),
          full: booleanValue(argumentsValue.full),
          minRewardCredit: decimalU64(argumentsValue.minRewardCredit),
          minSwapOutput: decimalU64(argumentsValue.minSwapOutput),
        },
      };
    case "dusk:settle_flash_liquidation":
      requireKeys(argumentsValue, []);
      return { specificationKey };
    case "dusk:preview_hlp_flash":
      requireKeys(argumentsValue, ["targetAsset"]);
      return {
        specificationKey,
        arguments: {
          targetAsset: unsignedNumber(argumentsValue.targetAsset, 0xff, "targetAsset"),
        },
      };
    case "dusk:begin_hlp_flash":
      requireKeys(argumentsValue, [
        "targetAsset",
        "maxPayment",
        "allowFull",
        "maxInsuranceDraw",
        "maxSocializedLoss",
        "maxCollateralDebit",
        "minCollateralCredit",
        "settleIndex",
      ]);
      return {
        specificationKey,
        arguments: {
          targetAsset: unsignedNumber(argumentsValue.targetAsset, 0xff, "targetAsset"),
          maxPayment: decimalU64(argumentsValue.maxPayment),
          allowFull: booleanValue(argumentsValue.allowFull),
          maxInsuranceDraw: decimalU64(argumentsValue.maxInsuranceDraw),
          maxSocializedLoss: decimalU64(argumentsValue.maxSocializedLoss),
          maxCollateralDebit: decimalU64(argumentsValue.maxCollateralDebit),
          minCollateralCredit: decimalU64(argumentsValue.minCollateralCredit),
          settleIndex: unsignedNumber(argumentsValue.settleIndex, 0xffff, "settleIndex"),
        },
      };
    case "dusk:settle_hlp_flash":
      requireKeys(argumentsValue, []);
      return { specificationKey };
    case "dusk:close_insolvent_hlp":
      requireKeys(argumentsValue, [
        "targetAsset",
        "maxInsuranceDraw",
        "maxSocializedLoss",
        "minCallerBountyOut",
      ]);
      return {
        specificationKey,
        arguments: {
          targetAsset: unsignedNumber(argumentsValue.targetAsset, 0xff, "targetAsset"),
          maxInsuranceDraw: decimalU64(argumentsValue.maxInsuranceDraw),
          maxSocializedLoss: decimalU64(argumentsValue.maxSocializedLoss),
          minCallerBountyOut: decimalU64(argumentsValue.minCallerBountyOut),
        },
      };

    case "dusk:queue_parameter_proposal":
    case "dusk:execute_parameter_proposal":
      requireKeys(argumentsValue, []);
      return { specificationKey };
    case "dusk:delegated_close_leverage":
      requireKeys(argumentsValue, [
        "debtAsset",
        "minAmountOut",
        "closeBps",
        "beforeIxDataHex",
        "afterIxDataHex",
        "beforeAccountsLen",
      ]);
      return {
        specificationKey,
        arguments: {
          debtAsset: unsignedNumber(
            argumentsValue.debtAsset,
            0xff,
            "debtAsset",
          ),
          closeBps: unsignedNumber(argumentsValue.closeBps, 0xffff, "closeBps"),
          minAmountOut: decimalU64(argumentsValue.minAmountOut),
          delegated: {
            beforeIxData: variableHex(argumentsValue.beforeIxDataHex),
            afterIxData: variableHex(argumentsValue.afterIxDataHex),
            beforeAccountsLen: unsignedNumber(
              argumentsValue.beforeAccountsLen,
              0xffff,
              "beforeAccountsLen",
            ),
          },
        },
      };
    case "leverage_delegate:before_take_profit":
    case "leverage_delegate:after_close_order":
    case "leverage_delegate:before_stop_loss":
      requireKeys(argumentsValue, ["orderId"]);
      return {
        specificationKey,
        arguments: { orderId: decimalU64(argumentsValue.orderId) },
      };
    case "dusk:settle_protocol_auction":
      requireKeys(argumentsValue, [
        "lane",
        "source",
        "soldAmount",
        "maxPaymentAmount",
      ]);
      return {
        specificationKey,
        arguments: {
          lane: enumValue(argumentsValue.lane, ["fee", "buyback"], "lane"),
          source: enumValue(
            argumentsValue.source,
            ["swap", "interest"],
            "source",
          ),
          soldAmount: decimalU64(argumentsValue.soldAmount),
          maxPaymentAmount: decimalU64(argumentsValue.maxPaymentAmount),
        },
      };
    default:
      throw encodingError(
        "unsupported_instruction",
        `${specificationKey}: unsupported keeper instruction`,
      );
  }
}

export function encodeKeeperInstructionJson(
  contract: InstructionContract,
  specificationKey: string,
  value: unknown,
): Uint8Array {
  return encodeKeeperInstruction(
    contract,
    parseKeeperInstructionArguments(specificationKey, value),
  );
}

class BorshWriter {
  readonly #chunks: Buffer[] = [];

  raw(value: Uint8Array): void {
    if (value.length !== 32)
      throw encodingError("invalid_arguments", "pubkey length");
    this.#chunks.push(Buffer.from(value));
  }

  u8(value: number): void {
    this.#unsigned(value, 0xff, 1, "u8");
  }

  u16(value: number): void {
    this.#unsigned(value, 0xffff, 2, "u16");
  }

  u32(value: number): void {
    this.#unsigned(value, 0xffff_ffff, 4, "u32");
  }

  u64(value: bigint): void {
    if (value < 0n || value > 0xffff_ffff_ffff_ffffn) {
      throw encodingError(
        "invalid_arguments",
        "u64 value is outside its Borsh range",
      );
    }
    const output = Buffer.alloc(8);
    output.writeBigUInt64LE(value);
    this.#chunks.push(output);
  }

  bytes(value: Uint8Array): void {
    if (value.byteLength > 0xffff_ffff) {
      throw encodingError(
        "invalid_arguments",
        "Borsh byte vector exceeds u32 length",
      );
    }
    this.u32(value.byteLength);
    this.#chunks.push(Buffer.from(value));
  }

  finish(): Buffer {
    return Buffer.concat(this.#chunks);
  }

  #unsigned(
    value: number,
    maximum: number,
    width: 1 | 2 | 4,
    label: string,
  ): void {
    if (!Number.isSafeInteger(value) || value < 0 || value > maximum) {
      throw encodingError(
        "invalid_arguments",
        `${label} value is outside its Borsh range`,
      );
    }
    const output = Buffer.alloc(width);
    if (width === 1) output.writeUInt8(value);
    if (width === 2) output.writeUInt16LE(value);
    if (width === 4) output.writeUInt32LE(value);
    this.#chunks.push(output);
  }
}

function requireRecord(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw encodingError(
      "invalid_arguments",
      "instruction arguments must be an object",
    );
  }
  return value as Record<string, unknown>;
}

function requireKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
): void {
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((entry, index) => entry !== wanted[index])
  ) {
    throw encodingError(
      "invalid_arguments",
      "instruction argument fields do not match the IDL",
    );
  }
}

function decimalU64(value: unknown): bigint {
  if (typeof value !== "string" || !/^(?:0|[1-9][0-9]*)$/.test(value)) {
    throw encodingError(
      "invalid_arguments",
      "u64 values must use canonical decimal strings",
    );
  }
  const parsed = BigInt(value);
  if (parsed > 0xffff_ffff_ffff_ffffn) {
    throw encodingError(
      "invalid_arguments",
      "u64 value is outside its Borsh range",
    );
  }
  return parsed;
}

function unsignedNumber(value: unknown, maximum: number, name: string): number {
  if (
    typeof value !== "number" ||
    !Number.isSafeInteger(value) ||
    value < 0 ||
    value > maximum
  ) {
    throw encodingError(
      "invalid_arguments",
      `${name} is outside its unsigned integer range`,
    );
  }
  return value as number;
}

function enumOrdinal<const T extends string>(
  value: T,
  allowed: readonly T[],
  name: string,
): number {
  const ordinal = allowed.indexOf(value);
  if (ordinal < 0) {
    throw encodingError(
      "invalid_arguments",
      `${name} is not a pinned IDL enum variant`,
    );
  }
  return ordinal;
}

function enumValue<const T extends string>(
  value: unknown,
  allowed: readonly T[],
  name: string,
): T {
  if (typeof value !== "string" || !allowed.includes(value as T)) {
    throw encodingError(
      "invalid_arguments",
      `${name} is not a pinned IDL enum variant`,
    );
  }
  return value as T;
}

function variableHex(value: unknown): Buffer {
  if (typeof value !== "string") {
    throw encodingError(
      "invalid_arguments",
      "byte vectors must use lowercase hex strings",
    );
  }
  return exactHex(value, null, "invalid_arguments");
}

function exactHex(
  value: string,
  bytes: number | null,
  code: InstructionEncodingErrorCode,
): Buffer {
  if (
    !/^(?:[0-9a-f]{2})*$/.test(value) ||
    (bytes !== null && value.length !== bytes * 2)
  ) {
    throw encodingError(
      code,
      "value is not the expected even-length lowercase hex",
    );
  }
  return Buffer.from(value, "hex");
}

function encodingError(
  code: InstructionEncodingErrorCode,
  message: string,
): InstructionEncodingError {
  return new InstructionEncodingError(code, message);
}

function booleanValue(value: unknown): boolean {
  if (typeof value !== "boolean")
    throw encodingError("invalid_arguments", "expected bool");
  return value;
}
function pubkeyValue(value: unknown): Uint8Array {
  try {
    if (typeof value !== "string") throw new Error();
    const bytes = decodePubkey(value, "invalid_seed_value");
    if (bytes.length !== 32) throw new Error();
    return bytes;
  } catch {
    throw encodingError("invalid_arguments", "expected pubkey");
  }
}
