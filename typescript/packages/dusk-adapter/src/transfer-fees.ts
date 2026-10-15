/** Independent native TypeScript reader for the SPL fee-notice contract.
 * Monitoring/prioritization only. Pinned on-chain previews decide eligibility.
 */
export const TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const TOKEN_2022_PROGRAM = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
export const CLOCK = "SysvarC1ock11111111111111111111111111111111";
export type Fee = { epoch: bigint; bps: number; maximum: bigint };
export type FeeSchedule = { older: Fee; newer: Fee };
export function feePhase(schedule: FeeSchedule | null, epoch: bigint): "none" | "notice" | "finalNotice" {
  if (epoch < 0n || epoch > (1n << 64n) - 1n) throw new Error("invalid epoch");
  if (!schedule || schedule.newer.epoch <= epoch) return "none";
  return schedule.newer.epoch - epoch === 1n ? "finalNotice" : "notice";
}
export function feeCredit(schedule: FeeSchedule | null, epoch: bigint, gross: bigint, mode: "effective" | "admission" | "liquidation"): bigint {
  if (gross < 0n || gross > (1n << 64n) - 1n) throw new Error("invalid amount");
  const phase = feePhase(schedule, epoch);
  if (!schedule) return gross;
  const credit = (f: Fee) => {
    if (!Number.isInteger(f.bps) || f.bps < 0 || f.bps > 10000 || f.maximum < 0n || f.maximum > (1n << 64n) - 1n) throw new Error("invalid transfer fee");
    const fee = (gross * BigInt(f.bps) + 9999n) / 10000n;
    return gross - (fee < f.maximum ? fee : f.maximum);
  };
  const current = credit(phase === "none" ? schedule.newer : schedule.older);
  const future = credit(schedule.newer);
  return phase !== "none" && (mode === "admission" || (mode === "liquidation" && phase === "finalNotice")) && future < current ? future : current;
}
export function decodeMintFees(owner: string, data: Buffer): FeeSchedule | null {
  if (![TOKEN_PROGRAM, TOKEN_2022_PROGRAM].includes(owner) || data.length < 82 || data[45] !== 1) throw new Error("invalid mint");
  if (data.length === 82) return null;
  if (owner === TOKEN_PROGRAM || data.length <= 165 || data.length === 355 || data[165] !== 1) throw new Error("invalid extended mint");
  let result: FeeSchedule | null = null;
  for (let offset = 166; offset < data.length;) {
    if (data.subarray(offset).every(byte => byte === 0)) break;
    if (offset + 4 > data.length) throw new Error("truncated TLV");
    const kind = data.readUInt16LE(offset), length = data.readUInt16LE(offset + 2); offset += 4;
    if (offset + length > data.length) throw new Error("truncated TLV");
    if (kind === 1) {
      if (result || length !== 108) throw new Error("invalid transfer fee config");
      const read = (at: number): Fee => {
        const fee = { epoch: data.readBigUInt64LE(at), maximum: data.readBigUInt64LE(at + 8), bps: data.readUInt16LE(at + 16) };
        if (fee.bps > 10000) throw new Error("invalid rate"); return fee;
      };
      result = { older: read(offset + 72), newer: read(offset + 90) };
    }
    offset += length;
  }
  return result;
}
export function decodeClockEpoch(owner: string, data: Buffer, slot: number): bigint {
  if (!Number.isSafeInteger(slot) || owner !== "Sysvar1111111111111111111111111111111111111" || data.length !== 40 || data.readBigUInt64LE(0) !== BigInt(slot)) throw new Error("invalid Clock snapshot");
  return data.readBigUInt64LE(16);
}
