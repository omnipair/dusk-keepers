// Read-only production capture. Never signs, publishes, freezes or waives gates.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile, copyFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Connection, PublicKey } from '@solana/web3.js';

const [sourceArg, bundleArg, sdkArg, revision] = process.argv.slice(2);
assert.equal(process.argv.length, 6, 'Expected CLEAN_CORE_CHECKOUT NORMAL_RELEASE_BUNDLE PUBLISHED_SDK_TGZ REVISION');
assert.match(revision, /^mainnet-\d{4}-\d{2}-\d{2}$/);
const source = resolve(sourceArg), sdk = resolve(sdkArg);
const git = (...args) => execFileSync('git', ['-C', source, ...args], { encoding: 'utf8' }).trim();
assert.equal(git('status', '--porcelain', '--untracked-files=no'), '', 'Core source must be clean');
const { readBundle, verifyOnChain, idlHash, decodeIdl, assertProgramBytes, CONFIG, LOADER } = await import(pathToFileURL(resolve(source, 'scripts/release/artifacts.mjs')));
const bundle = readBundle(resolve(bundleArg));
assert.equal(git('rev-parse', 'HEAD'), bundle.manifest.sourceCommit);
git('merge-base', '--is-ancestor', bundle.manifest.sourceCommit, 'origin/main');
const pkg = JSON.parse(execFileSync('tar', ['-xOf', sdk, 'package/package.json'], { encoding: 'utf8' }));
assert.equal(pkg.name, '@omnipair/dusk-sdk');
assert.equal(pkg.version, bundle.manifest.version);
assert.match(pkg.version, /^\d+\.\d+\.\d+$/);
const registry = await fetch(`https://registry.npmjs.org/@omnipair%2Fdusk-sdk/${pkg.version}`, { signal: AbortSignal.timeout(30_000) });
assert.ok(registry.ok, 'Published npm version metadata is unavailable');
const published = await registry.json();
assert.equal(published.name, pkg.name); assert.equal(published.version, pkg.version);
const dist = published.dist;
const integrity = 'sha512-' + createHash('sha512').update(await readFile(sdk)).digest('base64');
assert.equal(dist.integrity, integrity, 'SDK must be the exact npm-published tarball');
assert.ok(process.env.SOLANA_RPC_HTTP_URL, 'Explicit mainnet RPC is required');
const connection = new Connection(process.env.SOLANA_RPC_HTTP_URL, 'finalized');
assert.equal(await connection.getGenesisHash(), CONFIG.genesisHash);
const first = await verifyOnChain(connection, bundle, true);
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const json = value => JSON.stringify(value, null, 2) + '\n';
const programs = [];
await mkdir(resolve(source, 'target/idl'), { recursive: true });
await mkdir(resolve(source, 'target/deploy'), { recursive: true });
await mkdir(resolve(source, 'target/types'), { recursive: true });
for (const [name, entry] of Object.entries(bundle.manifest.programs)) {
  const program = new PublicKey(entry.id);
  const programData = PublicKey.findProgramAddressSync([program.toBuffer()], LOADER)[0];
  const [base] = PublicKey.findProgramAddressSync([], program);
  const idlAccount = await PublicKey.createWithSeed(base, 'anchor:idl', program);
  const observed = await connection.getMultipleAccountsInfoAndContext([program, programData, idlAccount], { commitment: 'finalized' });
  const [executable, data, rawIdl] = observed.value;
  assert.ok(executable?.executable && !data?.executable && rawIdl && data);
  assert.ok(executable.owner.equals(LOADER));
  assert.ok(data.owner.equals(executable.owner) && rawIdl.owner.equals(program));
  assert.equal(executable.data.readUInt32LE(0), 2);
  assert.ok(new PublicKey(executable.data.subarray(4, 36)).equals(programData));
  assert.equal(data.data.readUInt32LE(0), 3);
  assert.equal(data.data[12], 1);
  const authority = new PublicKey(data.data.subarray(13, 45)).toBase58();
  assert.equal(authority, CONFIG.deployer);
  assertProgramBytes(data.data, await readFile(resolve(bundle.directory, entry.binary.file)));
  const decoded = decodeIdl(rawIdl.data);
  const idl = JSON.parse(await readFile(resolve(bundle.directory, entry.idl.file), 'utf8'));
  assert.equal(decoded.authority, CONFIG.idlAuthority);
  assert.equal(idlHash(decoded.idl), idlHash(idl));
  const shipped = JSON.parse(execFileSync('tar', ['-xOf', sdk, name === 'dusk' ? 'package/dist/idl_v2.json' : 'package/dist/idl_delegate.json'], { encoding: 'utf8' }));
  assert.equal(idlHash(shipped), idlHash(idl), 'Published SDK IDL differs from deployed release');
  const binary = data.data.subarray(45), idlBytes = json(idl);
  await writeFile(resolve(source, `target/deploy/${name}.so`), binary);
  await writeFile(resolve(source, `target/idl/${name}.json`), idlBytes);
  await copyFile(resolve(bundle.directory, entry.types.file), resolve(source, `target/types/${name}.ts`));
  programs.push({ name, programId: entry.id, programData: programData.toBase58(), observedSlot: observed.context.slot,
    lastUpgradeSlot: Number(data.data.readBigUInt64LE(4)), binaryBytes: binary.length, binarySha256: hash(binary),
    idlAccount: idlAccount.toBase58(), idlAccountSha256: hash(rawIdl.data), idlSha256: hash(idlBytes),
    idlCanonicalSha256: idlHash(idl), upgradeAuthority: authority, idlAuthority: decoded.authority });
}
const last = await verifyOnChain(connection, bundle, true);
assert.deepEqual(last.programs.map(p => [p.name, p.deploymentSlot]), first.programs.map(p => [p.name, p.deploymentSlot]), 'Deployment changed during capture');
execFileSync(process.execPath, ['scripts/capture-protocol.mjs', source, sdk, revision], { stdio: 'inherit' });
const lock = JSON.parse(await readFile('protocol.lock.json', 'utf8'));
await writeFile('protocol/mainnet-deployment.v1.json', json({ schemaVersion: 1, cluster: 'mainnet-beta', genesisHash: CONFIG.genesisHash,
  capturedAt: lock.generatedAt, protocolRevision: revision, sourceCommit: bundle.manifest.sourceCommit,
  artifactOrigin: 'verified-production-release-and-upgradeable-loader-programdata',
  sourceVerification: 'Finalized executables, zero padding, IDLs and authorities match the exact production bundle. Remote source verification and the frozen live-release gate remain separate.',
  binaryEncoding: 'Entire ProgramData payload after the 45-byte metadata header, including allocation padding.',
  builderImage: bundle.manifest.builderImage, sdkIntegrity: integrity,
  replayFirstSlot: Math.max(...programs.map(p => p.lastUpgradeSlot)) + 1, programs }));
console.log('Mainnet captured, not frozen. Recapture SBF fixtures and complete every release gate before live signing.');
