# Mainnet keeper signing with AWS KMS

Each signing worker uses a separate non-exportable AWS KMS
`ECC_NIST_EDWARDS25519` key: lending, leverage, hLP recovery, auction arbitrage
and lifecycle. The sentinel has no signer. Native mainnet execution accepts
`SIGNER_PROVIDER=aws-kms`; local keys remain restricted to the explicit devnet
test exception.

Solana needs `ED25519_SHA_512` with `MessageType=RAW`. Ed25519ph is incompatible.
The runtime pins the full immutable key ARN and expected Solana public key,
validates key usage, checks returned key/algorithm identity and verifies each
signature locally before it can be broadcast. Denial or timeout abandons an
unsigned journal attempt; it never becomes a submitted transaction.

## Owner-local setup

1. Install current AWS CLI v2 and authenticate locally with
   `aws login --profile dusk-mainnet`, or the organization's IAM Identity Center
   profile. Choose the AWS region during setup. The owner identity needs key
   provisioning permissions; it must not become a worker's deployed identity.
2. Preview the provisioning plan, then run it with the chosen region:

   ```sh
   node scripts/provision-mainnet-kms.mjs --profile dusk-mainnet --region us-east-1 --output /path/to/keeper-kms-public.json
   node scripts/provision-mainnet-kms.mjs --profile dusk-mainnet --region us-east-1 --output /path/to/keeper-kms-public.json --execute
   ```

   The initial keys are provisioned in `us-east-1`. The script creates five keys, verifies their
   public keys and test signatures, and saves only public wallet identities,
   key ARNs and IAM policy documents. Preserve that manifest; reruns resume it
   instead of creating duplicate keys. It creates no market and transfers no SOL.
3. Give each service a distinct ECS task IAM role with only its generated IAM policy.
   Its key policy must also allow that principal. The policy permits only
   `kms:GetPublicKey` and pure-Ed25519 `kms:Sign` on that one key. Do not deploy
   owner/admin credentials or grant one worker access to the other workers' keys.
4. Configure its public variables from the manifest, mainnet RPC/database
   dependencies and the profile's economic limits. Configure AWS authentication
   through the ECS task-role credential provider. Do not create static IAM access
   keys for the workers. `AWS_REGION` names the key's region even when the
   container runs in another compute region. Never store a
   Solana keypair, AWS credentials or signer tokens in Git, logs or templates.
5. Fund the generated Solana public wallets for their approved operational
   budgets. Keep workers in shadow until the frozen artifact, parity, durable
   recovery and profile-specific release checks pass. Set each worker's spending
   bounds explicitly for the selected market/assets; a global raw-token amount
   is not a portable risk policy.

The release must retain the frozen-lock gate, direct account revalidation,
durable PostgreSQL lease/journal and submitted-unknown reconciliation. KMS is
key custody; it does not replace transaction simulation or economic policy.
IAM key/algorithm restrictions do not inspect Solana instruction contents.

The owner creates the first mainnet market through the app. Coordinate its
market/asset policy and the subsequent live canary before promotion; neither
this script nor the keeper setup creates a mainnet market.

Sources: [AWS Ed25519 key specifications](https://docs.aws.amazon.com/kms/latest/developerguide/symm-asymm-choose-key-spec.html),
[KMS signing condition keys](https://docs.aws.amazon.com/kms/latest/developerguide/conditions-kms.html),
[AWS CLI local login](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-sign-in.html).
