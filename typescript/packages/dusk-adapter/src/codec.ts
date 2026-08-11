import { createHash } from "node:crypto";

import type { InstructionContract } from "./envelope.ts";

export interface BidLiquidationAuctionArgs {
  readonly repayAmount: bigint;
  readonly minCollateralOut: bigint;
}

export interface SettleLiquidationAuctionFloorArgs {
  readonly repayAmount: bigint;
  readonly minCollateralOut: bigint;
  readonly maxInsuranceDraw: bigint;
  readonly maxSocializedLoss: bigint;
}

export interface LiquidateLeverageArgs {
  readonly debtAsset: number;
}

export interface DelegatedCpiArgs {
  readonly beforeIxData: Uint8Array;
  readonly afterIxData: Uint8Array;
  readonly beforeAccountsLen: number;
}

export interface DelegatedCloseLeverageArgs {
  readonly debtAsset: number;
  readonly minAmountOut: bigint;
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
  | { readonly specificationKey: "dusk:trigger_liquidation_auction" }
  | {
      readonly specificationKey: "dusk:bid_liquidation_auction";
      readonly arguments: BidLiquidationAuctionArgs;
    }
  | {
      readonly specificationKey: "dusk:settle_liquidation_auction_floor";
      readonly arguments: SettleLiquidationAuctionFloorArgs;
    }
  | {
      readonly specificationKey: "dusk:liquidate_leverage";
      readonly arguments: LiquidateLeverageArgs;
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
  const [expectedProgram, expectedName, unexpectedPart] = request.specificationKey.split(":");
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
  const discriminator = exactHex(specification.discriminatorHex, 8, "contract_mismatch");
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
    case "dusk:trigger_liquidation_auction":
    case "dusk:queue_parameter_proposal":
    case "dusk:execute_parameter_proposal":
      break;
    case "dusk:bid_liquidation_auction":
      writer.u64(request.arguments.repayAmount);
      writer.u64(request.arguments.minCollateralOut);
      break;
    case "dusk:settle_liquidation_auction_floor":
      writer.u64(request.arguments.repayAmount);
      writer.u64(request.arguments.minCollateralOut);
      writer.u64(request.arguments.maxInsuranceDraw);
      writer.u64(request.arguments.maxSocializedLoss);
      break;
    case "dusk:liquidate_leverage":
      writer.u8(request.arguments.debtAsset);
      break;
    case "dusk:delegated_close_leverage":
      writer.u8(request.arguments.debtAsset);
      writer.u64(request.arguments.minAmountOut);
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
      writer.u8(enumOrdinal(request.arguments.lane, ["fee", "buyback"], "lane"));
      writer.u8(enumOrdinal(request.arguments.source, ["swap", "interest"], "source"));
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
    case "dusk:trigger_liquidation_auction":
    case "dusk:queue_parameter_proposal":
    case "dusk:execute_parameter_proposal":
      requireKeys(argumentsValue, []);
      return { specificationKey };
    case "dusk:bid_liquidation_auction":
      requireKeys(argumentsValue, ["repayAmount", "minCollateralOut"]);
      return {
        specificationKey,
        arguments: {
          repayAmount: decimalU64(argumentsValue.repayAmount),
          minCollateralOut: decimalU64(argumentsValue.minCollateralOut),
        },
      };
    case "dusk:settle_liquidation_auction_floor":
      requireKeys(argumentsValue, [
        "repayAmount",
        "minCollateralOut",
        "maxInsuranceDraw",
        "maxSocializedLoss",
      ]);
      return {
        specificationKey,
        arguments: {
          repayAmount: decimalU64(argumentsValue.repayAmount),
          minCollateralOut: decimalU64(argumentsValue.minCollateralOut),
          maxInsuranceDraw: decimalU64(argumentsValue.maxInsuranceDraw),
          maxSocializedLoss: decimalU64(argumentsValue.maxSocializedLoss),
        },
      };
    case "dusk:liquidate_leverage":
      requireKeys(argumentsValue, ["debtAsset"]);
      return {
        specificationKey,
        arguments: { debtAsset: unsignedNumber(argumentsValue.debtAsset, 0xff, "debtAsset") },
      };
    case "dusk:delegated_close_leverage":
      requireKeys(argumentsValue, [
        "debtAsset",
        "minAmountOut",
        "beforeIxDataHex",
        "afterIxDataHex",
        "beforeAccountsLen",
      ]);
      return {
        specificationKey,
        arguments: {
          debtAsset: unsignedNumber(argumentsValue.debtAsset, 0xff, "debtAsset"),
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
      requireKeys(argumentsValue, ["lane", "source", "soldAmount", "maxPaymentAmount"]);
      return {
        specificationKey,
        arguments: {
          lane: enumValue(argumentsValue.lane, ["fee", "buyback"], "lane"),
          source: enumValue(argumentsValue.source, ["swap", "interest"], "source"),
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
  return encodeKeeperInstruction(contract, parseKeeperInstructionArguments(specificationKey, value));
}

class BorshWriter {
  readonly #chunks: Buffer[] = [];

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
      throw encodingError("invalid_arguments", "u64 value is outside its Borsh range");
    }
    const output = Buffer.alloc(8);
    output.writeBigUInt64LE(value);
    this.#chunks.push(output);
  }

  bytes(value: Uint8Array): void {
    if (value.byteLength > 0xffff_ffff) {
      throw encodingError("invalid_arguments", "Borsh byte vector exceeds u32 length");
    }
    this.u32(value.byteLength);
    this.#chunks.push(Buffer.from(value));
  }

  finish(): Buffer {
    return Buffer.concat(this.#chunks);
  }

  #unsigned(value: number, maximum: number, width: 1 | 2 | 4, label: string): void {
    if (!Number.isSafeInteger(value) || value < 0 || value > maximum) {
      throw encodingError("invalid_arguments", `${label} value is outside its Borsh range`);
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
    throw encodingError("invalid_arguments", "instruction arguments must be an object");
  }
  return value as Record<string, unknown>;
}

function requireKeys(value: Record<string, unknown>, expected: readonly string[]): void {
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((entry, index) => entry !== wanted[index])) {
    throw encodingError("invalid_arguments", "instruction argument fields do not match the IDL");
  }
}

function decimalU64(value: unknown): bigint {
  if (
    typeof value !== "string" ||
    !/^(?:0|[1-9][0-9]*)$/.test(value)
  ) {
    throw encodingError("invalid_arguments", "u64 values must use canonical decimal strings");
  }
  const parsed = BigInt(value);
  if (parsed > 0xffff_ffff_ffff_ffffn) {
    throw encodingError("invalid_arguments", "u64 value is outside its Borsh range");
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
    throw encodingError("invalid_arguments", `${name} is outside its unsigned integer range`);
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
    throw encodingError("invalid_arguments", `${name} is not a pinned IDL enum variant`);
  }
  return ordinal;
}

function enumValue<const T extends string>(
  value: unknown,
  allowed: readonly T[],
  name: string,
): T {
  if (typeof value !== "string" || !allowed.includes(value as T)) {
    throw encodingError("invalid_arguments", `${name} is not a pinned IDL enum variant`);
  }
  return value as T;
}

function variableHex(value: unknown): Buffer {
  if (typeof value !== "string") {
    throw encodingError("invalid_arguments", "byte vectors must use lowercase hex strings");
  }
  return exactHex(value, null, "invalid_arguments");
}

function exactHex(
  value: string,
  bytes: number | null,
  code: InstructionEncodingErrorCode,
): Buffer {
  if (!/^(?:[0-9a-f]{2})*$/.test(value) || (bytes !== null && value.length !== bytes * 2)) {
    throw encodingError(code, "value is not the expected even-length lowercase hex");
  }
  return Buffer.from(value, "hex");
}

function encodingError(
  code: InstructionEncodingErrorCode,
  message: string,
): InstructionEncodingError {
  return new InstructionEncodingError(code, message);
}
