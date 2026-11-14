// Owner-local provisioning. The output contains public identities and policies,
// never private keys, AWS credentials, or signer tokens.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { createPublicKey, verify } from 'node:crypto';
import { PublicKey } from '@solana/web3.js';

const args = process.argv.slice(2);
const value = flag => args[args.indexOf(flag) + 1];
const region = args.includes('--region') ? value('--region') : undefined;
const profile = args.includes('--profile') ? value('--profile') : undefined;
const output = args.includes('--output') ? resolve(value('--output')) : undefined;
assert.ok(region && /^[a-z]{2}-[a-z]+-\d$/.test(region), 'Pass --region with the chosen AWS region');
assert.ok(profile && !profile.startsWith('--'), 'Pass --profile with the owner-local AWS CLI profile');
assert.ok(output, 'Pass --output with an owner-local public manifest path');
const profiles = ['lending', 'leverage', 'hlp-recovery', 'auction-arbitrage', 'lifecycle'];
if (!args.includes('--execute')) {
  console.log(JSON.stringify({ region, profile, output, signingProfiles: profiles,
    actions: ['Create one non-exportable Ed25519 KMS key for each missing profile',
      'Verify each public key and a RAW Ed25519 test signature',
      'Write public wallet addresses and separate least-privilege IAM policies'],
    noMarketCreation: true, noSolTransfers: true }, null, 2));
  process.exit(0);
}

function aws(service, command, params = []) {
  try {
    return JSON.parse(execFileSync('aws', [service, command, ...params,
      '--profile', profile, '--region', region, '--output', 'json', '--no-cli-pager'],
      { encoding: 'utf8', timeout: 30_000, stdio: ['ignore', 'pipe', 'pipe'] }));
  } catch {
    throw new Error(`AWS ${service} ${command} failed. Check local login, permissions and CLI support. Provider output is withheld to protect credentials.`);
  }
}
const identity = aws('sts', 'get-caller-identity');
const manifest = existsSync(output) ? JSON.parse(readFileSync(output, 'utf8'))
  : { schemaVersion: 1, region, account: identity.Account, profiles: {} };
assert.equal(manifest.schemaVersion, 1);
assert.equal(manifest.region, region, 'Existing manifest belongs to another region');
assert.equal(manifest.account, identity.Account, 'Existing manifest belongs to another account');
mkdirSync(dirname(output), { recursive: true, mode: 0o700 });
const save = () => writeFileSync(output, JSON.stringify(manifest, null, 2) + '\n', { mode: 0o600 });

for (const name of profiles) {
  let entry = manifest.profiles[name];
  if (!entry) {
    const created = aws('kms', 'create-key', ['--key-spec', 'ECC_NIST_EDWARDS25519',
      '--key-usage', 'SIGN_VERIFY', '--description', `Dusk mainnet ${name} keeper`,
      '--tags', `TagKey=Application,TagValue=dusk-mainnet`, `TagKey=KeeperProfile,TagValue=${name}`]);
    entry = manifest.profiles[name] = { keyArn: created.KeyMetadata.Arn };
    // Persist immediately so interruption never silently creates another key.
    save();
  }
  const metadata = aws('kms', 'describe-key', ['--key-id', entry.keyArn]).KeyMetadata;
  assert.equal(metadata.Arn, entry.keyArn);
  assert.equal(metadata.KeySpec, 'ECC_NIST_EDWARDS25519');
  assert.equal(metadata.KeyUsage, 'SIGN_VERIFY');
  assert.equal(metadata.Enabled, true);
  const info = aws('kms', 'get-public-key', ['--key-id', entry.keyArn]);
  assert.equal(info.KeyId, entry.keyArn);
  assert.ok(info.SigningAlgorithms.includes('ED25519_SHA_512'));
  const der = Buffer.from(info.PublicKey, 'base64');
  assert.equal(der.length, 44);
  assert.equal(der.subarray(0, 12).toString('hex'), '302a300506032b6570032100');
  const publicKey = createPublicKey({ key: der, format: 'der', type: 'spki' });
  assert.equal(publicKey.asymmetricKeyType, 'ed25519');
  const address = new PublicKey(der.subarray(12)).toBase58();
  if (entry.publicKey) assert.equal(entry.publicKey, address, 'KMS wallet identity changed');
  const message = Buffer.from(`Dusk mainnet ${name} signer setup check`);
  const signature = aws('kms', 'sign', ['--key-id', entry.keyArn, '--message', message.toString('base64'),
    '--message-type', 'RAW', '--signing-algorithm', 'ED25519_SHA_512']);
  assert.equal(signature.KeyId, entry.keyArn);
  assert.equal(signature.SigningAlgorithm, 'ED25519_SHA_512');
  assert.ok(verify(null, message, publicKey, Buffer.from(signature.Signature, 'base64')), 'KMS signature did not verify');
  Object.assign(entry, { publicKey: address, verifiedAt: new Date().toISOString(), variables: {
    KEEPER_PROFILE: name, KEEPER_MODE: 'shadow', SIGNER_PROVIDER: 'aws-kms',
    SIGNER_KEY_ID: entry.keyArn, KEEPER_SIGNER_PUBLIC_KEY: address, AWS_REGION: region,
  }, iamPolicy: { Version: '2012-10-17', Statement: [
    { Effect: 'Allow', Action: ['kms:GetPublicKey'], Resource: entry.keyArn },
    { Effect: 'Allow', Action: ['kms:Sign'], Resource: entry.keyArn,
      Condition: { StringEquals: { 'kms:SigningAlgorithm': 'ED25519_SHA_512', 'kms:MessageType': 'RAW' } } },
  ] } });
  assert.equal(new Set(Object.values(manifest.profiles).map(p => p.keyArn)).size, Object.keys(manifest.profiles).length);
  const wallets = Object.values(manifest.profiles).map(p => p.publicKey).filter(Boolean);
  assert.equal(new Set(wallets).size, wallets.length);
  save();
  console.log(`${name}: ${address}`);
}
console.log(`Public identities and IAM policies saved to ${output}. No market or SOL transfer was created.`);
