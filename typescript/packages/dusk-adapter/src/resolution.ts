import { createHash } from "node:crypto";

import type {
  InstructionContract,
  JobIntent,
  ResolvedAccountMeta,
} from "./envelope.ts";
import type { ProtocolLock } from "./protocol-lock.ts";

const PDA_MARKER = Buffer.from("ProgramDerivedAddress", "utf8");
const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const FIELD_MODULUS = (1n << 255n) - 19n;
const EDWARDS_D = mod(-121665n * modInverse(121666n));
const SQRT_MINUS_ONE = modPow(2n, (FIELD_MODULUS - 1n) / 4n);

export interface AccountResolutionManifest {
  readonly schemaVersion: 1;
  readonly protocolRevision: string;
  readonly programs: readonly ResolutionProgram[];
  readonly staticAccounts: readonly StaticAccountSpecification[];
  readonly pdaRecipes: readonly PdaRecipe[];
}

export interface ResolutionProgram {
  readonly name: string;
  readonly programId: string;
  readonly idlSha256: string;
}

export type StaticAccountSource =
  | "idl_fixed_address"
  | "instruction_program"
  | "action_cpi_program";

export interface StaticAccountSpecification {
  readonly instructionKey: string;
  readonly accountName: string;
  readonly address: string;
  readonly source: StaticAccountSource;
}

export interface PdaRecipe {
  readonly key: string;
  readonly program: string;
  readonly programId: string;
  readonly usages: readonly PdaUsage[];
  readonly seeds: readonly PdaSeedRecipe[];
}

export interface PdaUsage {
  readonly instructionKey: string;
  readonly accountName: string;
  readonly idlSeedPaths: readonly string[];
}

export type PdaSeedEncoding = "pubkey" | "bytes32" | "u64_le";

export type PdaSeedRecipe =
  | { readonly kind: "const"; readonly valueHex: string }
  | { readonly kind: "input"; readonly name: string; readonly encoding: PdaSeedEncoding };

export type PdaSeedValue =
  | { readonly encoding: "pubkey"; readonly value: string }
  | { readonly encoding: "bytes32"; readonly value: Uint8Array }
  | { readonly encoding: "u64_le"; readonly value: bigint };

export interface ResolvedPda {
  readonly address: string;
  readonly bump: number;
}

export type AccountResolutionErrorCode =
  | "manifest_mismatch"
  | "unknown_recipe"
  | "missing_input"
  | "unexpected_input"
  | "invalid_seed_value"
  | "invalid_program_id"
  | "unable_to_find_pda";

export class AccountResolutionError extends Error {
  readonly code: AccountResolutionErrorCode;

  constructor(code: AccountResolutionErrorCode, message: string) {
    super(message);
    this.name = "AccountResolutionError";
    this.code = code;
  }
}

export class DeterministicAccountResolver {
  readonly #manifest: AccountResolutionManifest;

  constructor(
    lock: ProtocolLock,
    contract: InstructionContract,
    manifest: AccountResolutionManifest,
  ) {
    if (manifest.schemaVersion !== 1 || manifest.protocolRevision !== lock.revision) {
      throw resolutionError(
        "manifest_mismatch",
        "account resolution manifest revision differs from protocol lock",
      );
    }
    const programNames = new Set<string>();
    for (const program of manifest.programs) {
      if (programNames.has(program.name)) {
        throw resolutionError(
          "manifest_mismatch",
          "account resolution manifest has a duplicate program",
        );
      }
      programNames.add(program.name);
      const pinned = lock.programs.find((entry) => entry.name === program.name);
      if (
        !pinned ||
        pinned.programId !== program.programId ||
        pinned.idl.sha256 !== program.idlSha256
      ) {
        throw resolutionError(
          "manifest_mismatch",
          "manifest program provenance differs from protocol lock",
        );
      }
    }

    const staticKeys = new Set<string>();
    for (const account of manifest.staticAccounts) {
      const key = `${account.instructionKey}\0${account.accountName}`;
      const specification = contract.instructions.find(
        (entry) => entry.key === account.instructionKey,
      );
      if (
        staticKeys.has(key) ||
        !specification?.accounts.some((entry) => entry.name === account.accountName) ||
        !isPubkey(account.address)
      ) {
        throw resolutionError(
          "manifest_mismatch",
          "manifest contains an invalid or duplicate static account",
        );
      }
      staticKeys.add(key);
    }

    const recipeKeys = new Set<string>();
    for (const recipe of manifest.pdaRecipes) {
      const program = manifest.programs.find((entry) => entry.name === recipe.program);
      if (
        recipeKeys.has(recipe.key) ||
        recipe.seeds.length >= 16 ||
        !program ||
        program.programId !== recipe.programId ||
        !isPubkey(recipe.programId)
      ) {
        throw resolutionError("manifest_mismatch", "manifest contains an invalid PDA recipe");
      }
      recipeKeys.add(recipe.key);
      const inputNames = new Set<string>();
      for (const seed of recipe.seeds) {
        if (seed.kind === "const") {
          if (!/^(?:[0-9a-f]{2})+$/.test(seed.valueHex) || seed.valueHex.length > 64) {
            throw resolutionError(
              "manifest_mismatch",
              "PDA manifest contains invalid constant seed hex",
            );
          }
        } else if (inputNames.has(seed.name)) {
          throw resolutionError(
            "manifest_mismatch",
            "PDA manifest contains a duplicate input name",
          );
        } else {
          inputNames.add(seed.name);
        }
      }
      for (const usage of recipe.usages) {
        if (!contract.instructions.some((entry) => entry.key === usage.instructionKey)) {
          throw resolutionError(
            "manifest_mismatch",
            "PDA usage instruction is absent from contract",
          );
        }
      }
    }
    this.#manifest = manifest;
  }

  resolveStatic(instructionKey: string, accountName: string): string | undefined {
    return this.#manifest.staticAccounts.find(
      (entry) => entry.instructionKey === instructionKey && entry.accountName === accountName,
    )?.address;
  }

  pdaRecipeKey(instructionKey: string, accountName: string): string | undefined {
    return this.#manifest.pdaRecipes.find((recipe) =>
      recipe.usages.some(
        (usage) =>
          usage.instructionKey === instructionKey && usage.accountName === accountName,
      ),
    )?.key;
  }

  derivePda(recipeKey: string, inputs: ReadonlyMap<string, PdaSeedValue>): ResolvedPda {
    const recipe = this.#recipe(recipeKey);
    const expectedNames = new Set(
      recipe.seeds.flatMap((seed) => (seed.kind === "input" ? [seed.name] : [])),
    );
    for (const name of expectedNames) {
      if (!inputs.has(name)) {
        throw resolutionError("missing_input", `${recipeKey}: missing PDA input ${name}`);
      }
    }
    for (const name of inputs.keys()) {
      if (!expectedNames.has(name)) {
        throw resolutionError("unexpected_input", `${recipeKey}: unexpected PDA input ${name}`);
      }
    }

    const seeds = recipe.seeds.map((seed) => {
      if (seed.kind === "const") return Buffer.from(seed.valueHex, "hex");
      const value = inputs.get(seed.name);
      if (!value || value.encoding !== seed.encoding) {
        throw resolutionError(
          "invalid_seed_value",
          `${recipeKey}:${seed.name}: PDA input type differs from seed manifest`,
        );
      }
      if (value.encoding === "pubkey") return decodePubkey(value.value, "invalid_seed_value");
      if (value.encoding === "bytes32") {
        if (value.value.byteLength !== 32) {
          throw resolutionError(
            "invalid_seed_value",
            `${recipeKey}:${seed.name}: bytes32 input has the wrong length`,
          );
        }
        return Buffer.from(value.value);
      }
      if (value.value < 0n || value.value > 0xffff_ffff_ffff_ffffn) {
        throw resolutionError(
          "invalid_seed_value",
          `${recipeKey}:${seed.name}: u64 input is outside range`,
        );
      }
      const output = Buffer.alloc(8);
      output.writeBigUInt64LE(value.value);
      return output;
    });
    return findProgramAddress(seeds, recipe.programId);
  }

  derivePdaStrings(recipeKey: string, inputs: Readonly<Record<string, string>>): ResolvedPda {
    const recipe = this.#recipe(recipeKey);
    const expectedNames = new Set(
      recipe.seeds.flatMap((seed) => (seed.kind === "input" ? [seed.name] : [])),
    );
    for (const name of expectedNames) {
      if (!(name in inputs)) {
        throw resolutionError("missing_input", `${recipeKey}: missing PDA input ${name}`);
      }
    }
    for (const name of Object.keys(inputs)) {
      if (!expectedNames.has(name)) {
        throw resolutionError("unexpected_input", `${recipeKey}: unexpected PDA input ${name}`);
      }
    }
    const typed = new Map<string, PdaSeedValue>();
    for (const seed of recipe.seeds) {
      if (seed.kind !== "input") continue;
      const value = inputs[seed.name];
      if (seed.encoding === "pubkey") {
        decodePubkey(value, "invalid_seed_value");
        typed.set(seed.name, { encoding: "pubkey", value });
      } else if (seed.encoding === "bytes32") {
        if (!/^[0-9a-f]{64}$/.test(value)) {
          throw resolutionError(
            "invalid_seed_value",
            `${recipeKey}:${seed.name}: bytes32 input is invalid`,
          );
        }
        typed.set(seed.name, { encoding: "bytes32", value: Buffer.from(value, "hex") });
      } else {
        if (!/^(?:0|[1-9][0-9]*)$/.test(value)) {
          throw resolutionError(
            "invalid_seed_value",
            `${recipeKey}:${seed.name}: u64 input is not canonical decimal`,
          );
        }
        const parsed = BigInt(value);
        if (parsed > 0xffff_ffff_ffff_ffffn) {
          throw resolutionError(
            "invalid_seed_value",
            `${recipeKey}:${seed.name}: u64 input is outside range`,
          );
        }
        typed.set(seed.name, { encoding: "u64_le", value: parsed });
      }
    }
    return this.derivePda(recipeKey, typed);
  }

  #recipe(recipeKey: string): PdaRecipe {
    const recipe = this.#manifest.pdaRecipes.find((entry) => entry.key === recipeKey);
    if (!recipe) throw resolutionError("unknown_recipe", `${recipeKey}: unknown PDA recipe`);
    return recipe;
  }
}

function findProgramAddress(seeds: readonly Buffer[], programId: string): ResolvedPda {
  if (seeds.length >= 16 || seeds.some((seed) => seed.length > 32)) {
    throw resolutionError(
      "invalid_seed_value",
      "PDA seed count or length exceeds Solana limits",
    );
  }
  const program = decodePubkey(programId, "invalid_program_id");
  for (let bump = 255; bump >= 0; bump -= 1) {
    const digest = createHash("sha256")
      .update(Buffer.concat([...seeds, Buffer.from([bump]), program, PDA_MARKER]))
      .digest();
    if (!isEd25519Point(digest)) return { address: base58Encode(digest), bump };
  }
  throw resolutionError("unable_to_find_pda", "no off-curve PDA exists for the supplied seeds");
}

function isEd25519Point(bytes: Buffer): boolean {
  const y = mod(littleEndianBigInt(bytes) & ((1n << 255n) - 1n));
  const ySquared = mod(y * y);
  const numerator = mod(ySquared - 1n);
  const denominator = mod(EDWARDS_D * ySquared + 1n);
  if (denominator === 0n) return numerator === 0n;
  const ratio = mod(numerator * modInverse(denominator));
  let root = modPow(ratio, (FIELD_MODULUS + 3n) / 8n);
  if (mod(root * root) !== ratio) root = mod(root * SQRT_MINUS_ONE);
  return mod(root * root) === ratio;
}

function mod(value: bigint): bigint {
  const reduced = value % FIELD_MODULUS;
  return reduced < 0n ? reduced + FIELD_MODULUS : reduced;
}

function modPow(base: bigint, exponent: bigint): bigint {
  let result = 1n;
  let factor = mod(base);
  let remaining = exponent;
  while (remaining > 0n) {
    if ((remaining & 1n) === 1n) result = mod(result * factor);
    factor = mod(factor * factor);
    remaining >>= 1n;
  }
  return result;
}

function modInverse(value: bigint): bigint {
  return modPow(value, FIELD_MODULUS - 2n);
}

function littleEndianBigInt(bytes: Buffer): bigint {
  let value = 0n;
  for (let index = bytes.length - 1; index >= 0; index -= 1) {
    value = (value << 8n) | BigInt(bytes[index]);
  }
  return value;
}

function base58Encode(bytes: Buffer): string {
  const digits = [0];
  for (const byte of bytes) {
    let carry = byte;
    for (let index = 0; index < digits.length; index += 1) {
      const next = digits[index] * 256 + carry;
      digits[index] = next % 58;
      carry = Math.floor(next / 58);
    }
    while (carry > 0) {
      digits.push(carry % 58);
      carry = Math.floor(carry / 58);
    }
  }
  let output = "";
  for (let index = 0; index < bytes.length - 1 && bytes[index] === 0; index += 1) output += "1";
  return output + digits.reverse().map((digit) => BASE58_ALPHABET[digit]).join("");
}

function decodePubkey(
  value: string,
  errorCode: "invalid_seed_value" | "invalid_program_id",
): Buffer {
  let leadingZeros = 0;
  while (leadingZeros < value.length && value[leadingZeros] === "1") leadingZeros += 1;
  const bytes: number[] = [];
  for (const character of value.slice(leadingZeros)) {
    const digit = BASE58_ALPHABET.indexOf(character);
    if (digit < 0) throw resolutionError(errorCode, "public key is not base58");
    let carry = digit;
    for (let index = 0; index < bytes.length; index += 1) {
      const next = bytes[index] * 58 + carry;
      bytes[index] = next & 0xff;
      carry = next >> 8;
    }
    while (carry > 0) {
      bytes.push(carry & 0xff);
      carry >>= 8;
    }
  }
  const output = Buffer.concat([Buffer.alloc(leadingZeros), Buffer.from(bytes.reverse())]);
  if (output.length !== 32) throw resolutionError(errorCode, "public key must decode to 32 bytes");
  return output;
}

function isPubkey(value: string): boolean {
  try {
    decodePubkey(value, "invalid_seed_value");
    return true;
  } catch {
    return false;
  }
}

function resolutionError(code: AccountResolutionErrorCode, message: string): AccountResolutionError {
  return new AccountResolutionError(code, message);
}

export interface DynamicAccountRequirement {
  readonly name: string;
  readonly writable: boolean;
  readonly signer: boolean;
}

export interface DirectRpcAccountRequest {
  readonly intent: JobIntent;
  readonly specificationKey: string;
  readonly unresolvedAccounts: readonly DynamicAccountRequirement[];
  readonly minimumContextSlot: number;
}

export interface DirectRpcAccountSnapshot {
  readonly contextSlot: number;
  readonly accounts: readonly ResolvedAccountMeta[];
  readonly stateHash: string;
}

/** Implementations must fetch and decode safety-critical state directly from RPC. */
export interface DirectRpcDynamicAccountResolver {
  resolveAndRevalidate(request: DirectRpcAccountRequest): Promise<DirectRpcAccountSnapshot>;
}

export interface Token2022TransferLeg {
  readonly legId: string;
  readonly source: string;
  readonly mint: string;
  readonly destination: string;
  readonly authority: string;
  readonly amount: bigint;
  readonly decimals: number;
}

export interface Token2022RemainingAccountsRequest {
  readonly intent: JobIntent;
  readonly specificationKey: string;
  readonly transferLegs: readonly Token2022TransferLeg[];
  readonly minimumContextSlot: number;
}

export interface Token2022RemainingAccountGroup {
  readonly legId: string;
  readonly accounts: readonly ResolvedAccountMeta[];
}

export interface Token2022RemainingAccountsSnapshot {
  readonly contextSlot: number;
  readonly groups: readonly Token2022RemainingAccountGroup[];
}

/** Implementations resolve mint transfer-hook metas through direct RPC in exact leg order. */
export interface Token2022RemainingAccountResolver {
  resolveTransferHookAccounts(
    request: Token2022RemainingAccountsRequest,
  ): Promise<Token2022RemainingAccountsSnapshot>;
}
