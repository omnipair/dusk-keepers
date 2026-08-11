import { readFile } from "node:fs/promises";

export type LockStatus = "draft" | "captured" | "frozen";

export interface Artifact {
  readonly path: string;
  readonly sha256: string | null;
}

export interface PinnedProgram {
  readonly name: string;
  readonly programId: string | null;
  readonly binary: Artifact;
  readonly idl: Artifact;
}

export interface ProtocolLock {
  readonly schemaVersion: 1;
  readonly revision: string;
  readonly status: LockStatus;
  readonly generatedAt: string | null;
  readonly source: {
    readonly repository: string;
    readonly branch: string;
    readonly gitCommit: string;
    readonly worktreeFingerprintSha256: string | null;
  };
  readonly toolchain: Readonly<
    Record<"anchor" | "rust" | "solana" | "surfpool" | "node", string | null>
  >;
  readonly programs: readonly PinnedProgram[];
  readonly sdk: Artifact;
  readonly compatibility: Readonly<{
    accountLayoutFingerprintSha256: string | null;
    instructionFingerprintSha256: string | null;
    eventFingerprintSha256: string | null;
    errorFingerprintSha256: string | null;
    pdaFingerprintSha256: string | null;
  }>;
}

export async function readProtocolLock(path: string): Promise<ProtocolLock> {
  const value: unknown = JSON.parse(await readFile(path, "utf8"));
  return parseProtocolLock(value);
}

export function parseProtocolLock(value: unknown): ProtocolLock {
  if (!isRecord(value)) throw new Error("protocol lock must be an object");
  if (value.schemaVersion !== 1) throw new Error("unsupported protocol lock schema");
  if (typeof value.revision !== "string" || value.revision.length === 0) {
    throw new Error("protocol lock revision is missing");
  }
  if (value.status !== "draft" && value.status !== "captured" && value.status !== "frozen") {
    throw new Error("protocol lock status is invalid");
  }
  if (!Array.isArray(value.programs) || value.programs.length === 0) {
    throw new Error("protocol lock programs are missing");
  }
  return value as unknown as ProtocolLock;
}

export function assertLiveReady(lock: ProtocolLock): void {
  if (lock.status !== "frozen") throw new Error("protocol lock is not frozen");
  requireValue(lock.generatedAt, "generatedAt");
  requireHash(lock.source.worktreeFingerprintSha256, "source.worktreeFingerprintSha256");

  for (const [name, version] of Object.entries(lock.toolchain)) {
    requireValue(version, `toolchain.${name}`);
  }
  for (const program of lock.programs) {
    requireValue(program.programId, `${program.name}.programId`);
    requireHash(program.binary.sha256, `${program.name}.binary.sha256`);
    requireHash(program.idl.sha256, `${program.name}.idl.sha256`);
  }
  requireHash(lock.sdk.sha256, "sdk.sha256");
  for (const [name, hash] of Object.entries(lock.compatibility)) {
    requireHash(hash, `compatibility.${name}`);
  }
}

function requireValue(value: string | null, name: string): asserts value is string {
  if (!value || value.trim().length === 0) throw new Error(`protocol lock is missing ${name}`);
}

function requireHash(value: string | null, name: string): asserts value is string {
  if (!value || !/^[0-9a-f]{64}$/.test(value)) {
    throw new Error(`protocol lock has an invalid ${name}`);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
