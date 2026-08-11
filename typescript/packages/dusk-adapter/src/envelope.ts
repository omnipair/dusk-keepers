import { createHash } from "node:crypto";

import { assertLiveReady, type ProtocolLock } from "./protocol-lock.ts";

export type ProgramName = "dusk" | "leverage_delegate";

export type JobAction =
  | "lending_liquidation_trigger"
  | "lending_liquidation_bid"
  | "lending_liquidation_floor_settle"
  | "leverage_liquidation"
  | "delegated_close_take_profit"
  | "delegated_close_stop_loss"
  | "protocol_revenue_auction_settle"
  | "eligible_proposal_queue"
  | "eligible_proposal_execute";

export type JobKind =
  | "leverage_liquidation"
  | "lending_liquidation_trigger"
  | "lending_liquidation_bid"
  | "lending_liquidation_settle"
  | "stop_loss"
  | "take_profit"
  | "auction_bid"
  | "lifecycle"
  | "sentinel";

export interface JobIntent {
  readonly schemaVersion: 1;
  readonly intentId: string;
  readonly protocolRevision: string;
  readonly action: JobAction;
  readonly jobKind: JobKind;
  readonly market: string;
  readonly target: string;
  readonly observedSlot: number;
  readonly expectedStateHash: string;
}

export interface ResolvedAccountMeta {
  readonly name: string;
  readonly address: string;
  readonly writable: boolean;
  readonly signer: boolean;
}

export interface InstructionEnvelope {
  readonly specificationKey: string;
  readonly program: ProgramName;
  readonly programId: string;
  readonly instructionName: string;
  readonly dataHex: string;
  readonly accounts: readonly ResolvedAccountMeta[];
  readonly remainingAccounts: readonly ResolvedAccountMeta[];
}

export interface JobEnvelope {
  readonly schemaVersion: 1;
  readonly intent: JobIntent;
  readonly primary: InstructionEnvelope;
  readonly cpiHooks: readonly InstructionEnvelope[];
  readonly paritySha256: string;
}

export interface InstructionContract {
  readonly schemaVersion: 1;
  readonly protocolRevision: string;
  readonly programs: readonly ProgramContract[];
  readonly actions: readonly ActionContract[];
  readonly instructions: readonly InstructionSpecification[];
}

export interface ProgramContract {
  readonly name: ProgramName;
  readonly programId: string;
  readonly idlPath: string;
  readonly idlSha256: string;
}

export interface ActionContract {
  readonly action: JobAction;
  readonly jobKind: JobKind;
  readonly primaryInstructionKey: string;
  readonly cpiInstructionKeys: readonly string[];
  readonly requiredOptionalAccounts: readonly string[];
}

export interface InstructionSpecification {
  readonly key: string;
  readonly program: ProgramName;
  readonly programId: string;
  readonly instructionName: string;
  readonly discriminatorHex: string;
  readonly accounts: readonly AccountSpecification[];
}

export interface AccountSpecification {
  readonly name: string;
  readonly writable: boolean;
  readonly signer: boolean;
  readonly optional: boolean;
  readonly fixedAddress: string | null;
}

export class ValidatedJobEnvelope {
  readonly envelope: JobEnvelope;

  constructor(envelope: JobEnvelope) {
    this.envelope = envelope;
  }
}

const signableEnvelopeToken = Symbol("dusk.signable-envelope");

/** A validated envelope whose protocol lock also passed every live-readiness pin. */
export class SignableJobEnvelope {
  readonly envelope: JobEnvelope;

  constructor(envelope: JobEnvelope, token: symbol) {
    if (token !== signableEnvelopeToken) {
      throw new TypeError("signable envelopes may only be created by EnvelopeValidator");
    }
    this.envelope = envelope;
  }
}

export type EnvelopeValidationErrorCode =
  | "revision_mismatch"
  | "job_kind_mismatch"
  | "instruction_mismatch"
  | "program_id_mismatch"
  | "invalid_pubkey"
  | "invalid_data_hex"
  | "discriminator_mismatch"
  | "missing_required_account"
  | "account_order_mismatch"
  | "account_flags_mismatch"
  | "fixed_address_mismatch"
  | "required_optional_account_missing"
  | "cpi_hook_mismatch"
  | "remaining_accounts_mismatch"
  | "delegated_hook_data_mismatch"
  | "parity_mismatch"
  | "live_protocol_not_ready";

export class EnvelopeValidationError extends Error {
  readonly code: EnvelopeValidationErrorCode;

  constructor(code: EnvelopeValidationErrorCode, message: string) {
    super(message);
    this.name = "EnvelopeValidationError";
    this.code = code;
  }
}

export class EnvelopeValidator {
  readonly #lock: ProtocolLock;
  readonly #contract: InstructionContract;

  constructor(lock: ProtocolLock, contract: InstructionContract) {
    if (contract.schemaVersion !== 1 || contract.protocolRevision !== lock.revision) {
      throw validationError(
        "revision_mismatch",
        "instruction contract revision does not match protocol lock",
      );
    }
    const names = new Set<ProgramName>();
    for (const program of contract.programs) {
      if (names.has(program.name)) {
        throw validationError(
          "instruction_mismatch",
          "instruction contract contains a duplicate program",
        );
      }
      names.add(program.name);
      const pinned = lock.programs.find((candidate) => candidate.name === program.name);
      if (
        !pinned ||
        pinned.programId !== program.programId ||
        pinned.idl.path !== program.idlPath ||
        pinned.idl.sha256 !== program.idlSha256
      ) {
        throw validationError(
          "program_id_mismatch",
          "instruction contract program provenance differs from protocol lock",
        );
      }
    }
    this.#lock = lock;
    this.#contract = contract;
  }

  validate(envelope: JobEnvelope): ValidatedJobEnvelope {
    if (envelope.schemaVersion !== 1 || envelope.intent.schemaVersion !== 1) {
      throw validationError("instruction_mismatch", "unsupported envelope schema");
    }
    if (envelope.intent.protocolRevision !== this.#lock.revision) {
      throw validationError("revision_mismatch", "job intent revision does not match protocol lock");
    }
    const action = this.#contract.actions.find(
      (candidate) => candidate.action === envelope.intent.action,
    );
    if (!action) {
      throw validationError("instruction_mismatch", "job action is absent from instruction contract");
    }
    if (envelope.intent.jobKind !== action.jobKind) {
      throw validationError("job_kind_mismatch", "job kind does not match action");
    }

    this.#validateExpectedInstruction(envelope.primary, action.primaryInstructionKey);
    if (
      envelope.cpiHooks.length !== action.cpiInstructionKeys.length ||
      envelope.cpiHooks.some(
        (hook, index) => hook.specificationKey !== action.cpiInstructionKeys[index],
      )
    ) {
      throw validationError("cpi_hook_mismatch", "CPI hook sequence does not match job action");
    }
    for (let index = 0; index < envelope.cpiHooks.length; index += 1) {
      const hook = envelope.cpiHooks[index];
      this.#validateExpectedInstruction(hook, action.cpiInstructionKeys[index]);
      if (hook.remainingAccounts.length > 0) {
        throw validationError(
          "cpi_hook_mismatch",
          "nested CPI hook remaining accounts are not supported",
        );
      }
    }

    for (const required of action.requiredOptionalAccounts) {
      if (!envelope.primary.accounts.some((account) => account.name === required)) {
        throw validationError(
          "required_optional_account_missing",
          `job action requires optional IDL account ${required}`,
        );
      }
    }
    if (action.cpiInstructionKeys.length > 0) this.#validateDelegatedComposition(envelope);

    if (envelopeParitySha256(envelope) !== envelope.paritySha256) {
      throw validationError("parity_mismatch", "canonical byte/account-order digest differs");
    }
    return new ValidatedJobEnvelope(envelope);
  }

  /** Returns the only envelope type accepted by the signing port. */
  validateForSigning(envelope: JobEnvelope): SignableJobEnvelope {
    try {
      assertLiveReady(this.#lock);
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      throw validationError(
        "live_protocol_not_ready",
        `protocol lock is not ready for signing: ${detail}`,
      );
    }
    this.validate(envelope);
    return new SignableJobEnvelope(envelope, signableEnvelopeToken);
  }

  #validateExpectedInstruction(envelope: InstructionEnvelope, expectedKey: string): void {
    if (envelope.specificationKey !== expectedKey) {
      throw validationError("instruction_mismatch", "instruction specification key differs");
    }
    const specification = this.#contract.instructions.find(
      (candidate) => candidate.key === expectedKey,
    );
    if (!specification) {
      throw validationError("instruction_mismatch", "instruction specification is absent");
    }
    if (
      envelope.program !== specification.program ||
      envelope.instructionName !== specification.instructionName
    ) {
      throw validationError(
        "instruction_mismatch",
        "program or instruction name differs from IDL contract",
      );
    }
    if (envelope.programId !== specification.programId) {
      throw validationError("program_id_mismatch", "program ID differs from pinned contract");
    }
    requirePubkey(envelope.programId);
    const data = decodeHex(envelope.dataHex);
    const discriminator = decodeHex(specification.discriminatorHex);
    if (!data.subarray(0, discriminator.length).equals(discriminator)) {
      throw validationError(
        "discriminator_mismatch",
        "instruction data does not start with the pinned Anchor discriminator",
      );
    }
    this.#validateAccounts(envelope, specification);
    for (const account of envelope.remainingAccounts) requirePubkey(account.address);
  }

  #validateAccounts(
    envelope: InstructionEnvelope,
    specification: InstructionSpecification,
  ): void {
    let actualIndex = 0;
    for (const expected of specification.accounts) {
      const actual = envelope.accounts[actualIndex];
      if (expected.optional && (!actual || actual.name !== expected.name)) continue;
      if (!actual) {
        throw validationError(
          "missing_required_account",
          `missing required account ${expected.name}`,
        );
      }
      if (actual.name !== expected.name) {
        const expectedIsLater = envelope.accounts
          .slice(actualIndex + 1)
          .some((account) => account.name === expected.name);
        throw validationError(
          expectedIsLater ? "account_order_mismatch" : "missing_required_account",
          `expected account ${expected.name}, got ${actual.name}`,
        );
      }
      requirePubkey(actual.address);
      if (actual.writable !== expected.writable || actual.signer !== expected.signer) {
        throw validationError(
          "account_flags_mismatch",
          `account ${expected.name} flags differ from IDL`,
        );
      }
      const fixedAddress = expected.fixedAddress ??
        (expected.name === "program" ? specification.programId : null);
      if (fixedAddress && fixedAddress !== actual.address) {
        throw validationError(
          "fixed_address_mismatch",
          `account ${expected.name} differs from its fixed address`,
        );
      }
      actualIndex += 1;
    }
    if (actualIndex !== envelope.accounts.length) {
      throw validationError(
        "account_order_mismatch",
        "instruction envelope has unexpected base accounts",
      );
    }
  }

  #validateDelegatedComposition(envelope: JobEnvelope): void {
    if (envelope.cpiHooks.length !== 2) {
      throw validationError("cpi_hook_mismatch", "delegated close requires exactly two CPI hooks");
    }
    const primary = decodeHex(envelope.primary.dataHex);
    const before = decodeHex(envelope.cpiHooks[0].dataHex);
    const after = decodeHex(envelope.cpiHooks[1].dataHex);
    const embedded = decodeDelegatedCpiPayload(primary);
    if (!embedded.before.equals(before) || !embedded.after.equals(after)) {
      throw validationError(
        "delegated_hook_data_mismatch",
        "delegated close embedded CPI bytes differ from hook envelopes",
      );
    }
    if (embedded.beforeAccountsLength !== envelope.cpiHooks[0].accounts.length) {
      throw validationError(
        "remaining_accounts_mismatch",
        "before_accounts_len differs from before-hook account count",
      );
    }
    const expectedRemaining = envelope.cpiHooks.flatMap((hook) => hook.accounts);
    if (
      expectedRemaining.length !== envelope.primary.remainingAccounts.length ||
      expectedRemaining.some(
        (account, index) => !sameAccount(account, envelope.primary.remainingAccounts[index]),
      )
    ) {
      throw validationError(
        "remaining_accounts_mismatch",
        "primary remaining accounts are not the exact before/after hook concatenation",
      );
    }
    const delegatedProgram = envelope.primary.accounts.find(
      (account) => account.name === "delegated_program",
    );
    if (!delegatedProgram || delegatedProgram.address !== envelope.cpiHooks[0].programId) {
      throw validationError(
        "fixed_address_mismatch",
        "delegated_program does not match CPI hook program",
      );
    }
  }
}

function sameAccount(left: ResolvedAccountMeta, right: ResolvedAccountMeta): boolean {
  return (
    left.name === right.name &&
    left.address === right.address &&
    left.writable === right.writable &&
    left.signer === right.signer
  );
}

function decodeDelegatedCpiPayload(data: Buffer): {
  before: Buffer;
  after: Buffer;
  beforeAccountsLength: number;
} {
  if (data.length < 27) throw invalidDelegatedData();
  let cursor = 17;
  const before = readVector(data, cursor);
  cursor = before.next;
  const after = readVector(data, cursor);
  cursor = after.next;
  if (cursor + 2 !== data.length) throw invalidDelegatedData();
  return {
    before: before.bytes,
    after: after.bytes,
    beforeAccountsLength: data.readUInt16LE(cursor),
  };
}

function readVector(data: Buffer, cursor: number): { bytes: Buffer; next: number } {
  if (cursor + 4 > data.length) throw invalidDelegatedData();
  const length = data.readUInt32LE(cursor);
  const start = cursor + 4;
  const end = start + length;
  if (end > data.length) throw invalidDelegatedData();
  return { bytes: data.subarray(start, end), next: end };
}

function invalidDelegatedData(): EnvelopeValidationError {
  return validationError(
    "delegated_hook_data_mismatch",
    "delegated close CPI payload is malformed",
  );
}

function requirePubkey(address: string): void {
  const alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  let leadingZeros = 0;
  while (leadingZeros < address.length && address[leadingZeros] === "1") leadingZeros += 1;
  const bytes: number[] = [];
  for (const character of address.slice(leadingZeros)) {
    const digit = alphabet.indexOf(character);
    if (digit < 0) throw validationError("invalid_pubkey", "account address is not base58");
    let carry = digit;
    for (let index = 0; index < bytes.length; index += 1) {
      const value = bytes[index] * 58 + carry;
      bytes[index] = value & 0xff;
      carry = value >> 8;
    }
    while (carry > 0) {
      bytes.push(carry & 0xff);
      carry >>= 8;
    }
  }
  if (leadingZeros + bytes.length !== 32) {
    throw validationError("invalid_pubkey", "account address does not decode to 32 bytes");
  }
}

function decodeHex(value: string): Buffer {
  if (value.length < 16 || value.length % 2 !== 0 || !/^[0-9a-f]+$/.test(value)) {
    throw validationError(
      "invalid_data_hex",
      "instruction data must be lowercase, even-length hex with an 8-byte discriminator",
    );
  }
  return Buffer.from(value, "hex");
}

class CanonicalWriter {
  readonly #chunks: Buffer[] = [];

  u8(value: number): void {
    this.#chunks.push(Buffer.from([value]));
  }

  u32(value: number): void {
    if (!Number.isSafeInteger(value) || value < 0 || value > 0xffff_ffff) {
      throw validationError("parity_mismatch", "canonical field exceeds u32 length");
    }
    const buffer = Buffer.alloc(4);
    buffer.writeUInt32LE(value);
    this.#chunks.push(buffer);
  }

  u64(value: number): void {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw validationError("parity_mismatch", "slot is outside safe integer range");
    }
    const buffer = Buffer.alloc(8);
    buffer.writeBigUInt64LE(BigInt(value));
    this.#chunks.push(buffer);
  }

  string(value: string): void {
    const bytes = Buffer.from(value, "utf8");
    this.u32(bytes.length);
    this.#chunks.push(bytes);
  }

  bytes(value: Buffer): void {
    this.u32(value.length);
    this.#chunks.push(value);
  }

  account(account: ResolvedAccountMeta): void {
    this.string(account.name);
    this.string(account.address);
    this.u8(account.writable ? 1 : 0);
    this.u8(account.signer ? 1 : 0);
  }

  instruction(instruction: InstructionEnvelope): void {
    this.string(instruction.specificationKey);
    this.string(instruction.program);
    this.string(instruction.programId);
    this.string(instruction.instructionName);
    this.bytes(decodeHex(instruction.dataHex));
    this.u32(instruction.accounts.length);
    for (const account of instruction.accounts) this.account(account);
    this.u32(instruction.remainingAccounts.length);
    for (const account of instruction.remainingAccounts) this.account(account);
  }

  finish(): Buffer {
    return Buffer.concat(this.#chunks);
  }
}

export function envelopeParitySha256(envelope: JobEnvelope): string {
  const writer = new CanonicalWriter();
  writer.u8(envelope.schemaVersion);
  writer.u8(envelope.intent.schemaVersion);
  writer.string(envelope.intent.intentId);
  writer.string(envelope.intent.protocolRevision);
  writer.string(envelope.intent.action);
  writer.string(envelope.intent.jobKind);
  writer.string(envelope.intent.market);
  writer.string(envelope.intent.target);
  writer.u64(envelope.intent.observedSlot);
  writer.string(envelope.intent.expectedStateHash);
  writer.instruction(envelope.primary);
  writer.u32(envelope.cpiHooks.length);
  for (const hook of envelope.cpiHooks) writer.instruction(hook);
  return createHash("sha256").update(writer.finish()).digest("hex");
}

function validationError(
  code: EnvelopeValidationErrorCode,
  message: string,
): EnvelopeValidationError {
  return new EnvelopeValidationError(code, message);
}

export interface EnvelopeSigner<SignedTransaction> {
  sign(envelope: SignableJobEnvelope): Promise<SignedTransaction>;
}
