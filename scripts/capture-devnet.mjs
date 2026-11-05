// Capture the deployed ABI and executable payloads, never sign or upgrade.
// Use an isolated clean source checkout whose vendored IDLs match the chain.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { inflateSync } from "node:zlib";
import { Connection, PublicKey } from "@solana/web3.js";

const [sourceArg, sdkArg, revision] = process.argv.slice(2);
assert.ok(sourceArg && sdkArg && revision,
  "usage: capture-devnet.mjs <clean-source-checkout> <packed-sdk.tgz> <revision>");
const source = resolve(sourceArg);
const git = (...args) => execFileSync("git", ["-C", source, ...args], { encoding: "utf8" }).trim();
assert.equal(git("status", "--porcelain", "--untracked-files=no"), "", "source must be clean");
const connection = new Connection(process.env.SOLANA_RPC_HTTP_URL ?? "https://api.devnet.solana.com", "confirmed");
const genesisHash = await connection.getGenesisHash();
assert.equal(genesisHash, "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG", "capture requires Solana devnet");
const loader = "BPFLoaderUpgradeab1e11111111111111111111111";
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
const json = value => JSON.stringify(value, null, 2) + "\n";
const programs = [];
await mkdir(`${source}/target/idl`, { recursive: true });
await mkdir(`${source}/target/deploy`, { recursive: true });

for (const [name, sourceIdlPath] of [
  ["dusk", "packages/dusk-sdk/src/idl_v2.json"],
  ["leverage_delegate", "packages/dusk-sdk/src/idl_delegate.json"],
]) {
  const sourceIdl = JSON.parse(await readFile(`${source}/${sourceIdlPath}`, "utf8"));
  const program = new PublicKey(sourceIdl.address);
  const [base] = PublicKey.findProgramAddressSync([], program);
  const idlAddress = await PublicKey.createWithSeed(base, "anchor:idl", program);
  const initial = await connection.getMultipleAccountsInfoAndContext([program, idlAddress]);
  const [executable, idlAccount] = initial.value;
  assert.ok(executable?.executable && executable.owner.toBase58() === loader, `${name}: invalid executable`);
  assert.equal(executable.data.readUInt32LE(0), 2, `${name}: not an upgradeable Program`);
  assert.ok(idlAccount && !idlAccount.executable && idlAccount.owner.equals(program), `${name}: invalid IDL owner`);
  const programData = new PublicKey(executable.data.subarray(4, 36));
  const deployed = await connection.getAccountInfoAndContext(programData, { minContextSlot: initial.context.slot });
  assert.ok(deployed.value && !deployed.value.executable && deployed.value.owner.toBase58() === loader);
  const data = deployed.value.data;
  assert.ok(data.length > 45 && data.readUInt32LE(0) === 3, `${name}: invalid ProgramData`);
  const binary = data.subarray(45); // Retain all allocation padding; hash exactly these chain bytes.
  assert.ok(binary.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46])), `${name}: not ELF`);
  const compressedLength = idlAccount.data.readUInt32LE(40);
  assert.ok(compressedLength > 0 && 44 + compressedLength <= idlAccount.data.length, `${name}: truncated IDL`);
  const idl = JSON.parse(inflateSync(idlAccount.data.subarray(44, 44 + compressedLength)).toString("utf8"));
  assert.deepEqual(idl, sourceIdl, `${name}: source IDL differs from deployed IDL`);
  const recheck = await connection.getMultipleAccountsInfoAndContext([program, idlAddress, programData], {
    minContextSlot: deployed.context.slot,
  });
  for (const [index, original] of [executable, idlAccount, deployed.value].entries()) {
    assert.ok(recheck.value[index]?.owner.equals(original.owner)
      && recheck.value[index]?.executable === original.executable
      && recheck.value[index]?.data.equals(original.data), `${name}: deployment changed during capture`);
  }
  const idlBytes = json(idl);
  await writeFile(`${source}/target/idl/${name}.json`, idlBytes);
  await writeFile(`${source}/target/deploy/${name}.so`, binary);
  programs.push({
    name, programId: program.toBase58(), programData: programData.toBase58(),
    observedSlot: recheck.context.slot, lastUpgradeSlot: Number(data.readBigUInt64LE(4)),
    binaryBytes: binary.length, binarySha256: sha(binary),
    idlAccount: idlAddress.toBase58(), idlSha256: sha(idlBytes),
    idlAccountSha256: sha(idlAccount.data),
  });
  console.log(`verified ${name} ${program} at upgrade slot ${programs.at(-1).lastUpgradeSlot}`);
}

execFileSync(process.execPath, ["scripts/capture-protocol.mjs", source, resolve(sdkArg), revision], { stdio: "inherit" });
const lock = JSON.parse(await readFile("protocol.lock.json", "utf8"));
await writeFile("protocol/devnet-deployment.v1.json", json({
  schemaVersion: 1, cluster: "devnet", genesisHash, capturedAt: lock.generatedAt,
  protocolRevision: lock.revision, sourceCommit: git("rev-parse", "HEAD"),
  artifactOrigin: "upgradeable-loader-programdata",
  sourceVerification: "Vendored IDLs match the deployed IDLs exactly. Source is used for SDK and fixture construction; this is not a reproducible-build attestation.",
  binaryEncoding: "Entire ProgramData payload after its 45-byte metadata header, including allocation padding.",
  programs,
}));
console.log("Devnet capture recorded. Recapture bank fixtures against these deployed bytes before committing.");
