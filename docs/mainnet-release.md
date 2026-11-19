# Mainnet keeper preparation

The `mainnet-2026-11-19` candidate ports both independent adapters to the exact
production ABI from `aa196e3423fd5f46b00630a4f161d91f95ef7b3d`. LP interest is
credited through the accrued index; removed interest-vault and IOU accounts are
not included in liquidation, hLP, delegated-order or auction plans. Active hLP
funding uses the three-account srLP mint/vault prefix. Oracle accounts follow it.

## Current evidence

The actual finalized release is frozen under the separate
[2026-11-19 release-test waiver](evidence/mainnet-release-waiver.json), following
the owner's existing instruction to skip release tests and their request to
prepare local live execution on the new market. The full release matrix and
mainnet canary remain incomplete. The SDK is the exact
registry-published `3.0.1` archive (SHA-256
`70eca34c89a1e80f64c3907e5f92c3196c3dfba01a38d8e75465a7c529b63ef6`).
Dusk finalized at slot 471276209 and delegate at 471275760; the complete
revision begins at 471276210. Both on-chain programs, IDLs and deployer
upgrade/IDL authorities match the tested production bundle, and both independent
source-verification jobs match the exact commit. The local release decision
does not start any workers or activate hosted services.

The former release, its receipt and scoped waiver are archived under
`artifacts/releases/mainnet-2026-11-14`. That waiver applies only to its original
revision. The current decision records the new revision and hashes separately.

The new market is `GWrmPqBcmBgX3fcE3GPA6xgCUP3Cbd4DRvXHyfwu1795` (layout 19,
4,478 bytes). The former `7xf68VEvMtLJ2Up5zVZdA161uH1yNkTFYgDFhNUSwkM7` is
retired on-chain. Normal discovery includes the new market without a market filter.

## Capture the actual release

After the core upgrade, both IDL uploads, exact on-chain verification and stable
SDK publication, download the exact npm tarball and run:

```sh
node scripts/capture-mainnet.mjs CLEAN_CORE_CHECKOUT NORMAL_RELEASE_BUNDLE \
  PUBLISHED_SDK_TGZ mainnet-2026-11-19
node scripts/capture-liquidation-fixtures.mjs CLEAN_CORE_CHECKOUT
node scripts/capture-hlp-fixtures.mjs CLEAN_CORE_CHECKOUT
cargo build --locked -p dusk-keeper
node scripts/validate-repo.mjs
cargo test --locked --workspace
npm run typecheck
npm test
```

The read-only capture verifies the SDK's registry integrity, deployed ELF and
zero padding, both IDLs and their authorities against the reviewed production
bundle. It retains full ProgramData payloads and actual upgrade slots; it never
invents a receipt, signs, freezes the lock or inherits a waiver. Rebuild fixture
receipts against those captured bytes. The common replay boundary follows both
actual upgrades; the retired market's canonical payout history is separate.

The complete new [Surfpool release matrix](runbooks/surfpool-e2e.md) remains
waived for this local release decision. Captured liquidation/hLP rehearsals alone
do not cover TP/SL, auctions, lifecycle and all fault scenarios. Do not record
those scenarios or a mainnet canary as passed. Hosted activation is separate;
routine code, artifact and live dependency checks remain required.

## AWS service parity

Deploy separate Rust services for lending, leverage, hLP recovery, auction
arbitrage, lifecycle and sentinel, preserving the current Railway service
counts, profiles and intervals. Keep them in shadow during infrastructure
migration. The frontend stays on Vercel and v1 stays on its current stack.
Reuse the existing Helius subscription and the matching mainnet indexer.

Five distinct non-exportable Ed25519 KMS keys are provisioned in `us-east-1`.
Each signing service needs its own ECS task IAM role restricted to its key;
sentinel has no wallet. Follow [the KMS runbook](runbooks/mainnet-kms.md).
Do not put owner credentials or Solana private keys in the deployment.

Before activation, configure verified database TLS (or the private local Unix
socket), durable schema migrations and recovery, signing policies, wallet funding
and explicit execution settings. Neither this release nor the KMS provisioning
script creates or funds a market. No production canary is claimed by an offline fixture receipt.

## Spending configuration

Start from [`deploy/mainnet/env.example`](../deploy/mainnet/env.example) and a
reviewed copy of [`risk-policy.example.json`](../deploy/mainnet/risk-policy.example.json).
The examples stay in shadow and are not automatically loaded. Inventory payments
have **no per-token transaction or daily spending cap**, and no payment-mint
allowlist. These fee limits remain **per signing wallet**:

| Asset or expense | Per transaction | Per UTC day |
| --- | ---: | ---: |
| SOL transaction fees, including priority | 0.0002 SOL | 0.02 SOL |

Each wallet retains at least 0.05 SOL after simulated rent/account creation and
fees. Rent is not part of the daily fee total. The reserve check conservatively
subtracts the quoted fee from the simulated payer balance even if the RPC already
deducted it. Fund inventory separately from this SOL reserve. Zero-bounty orders
remain supported; their fees still consume the daily fee allowance.

Amounts in the policy are decimal strings of raw atoms. The policy contains only
`fees` and `minimumSolReserve`; remove the old `tokens` entries when updating an
earlier copy. Inventory purchases work across all supported markets using available
wallet tokens and the protocol's permitted repayment. The lending/leverage example
sets the legacy global repayment/payment bounds to `u64::MAX`, so they add no
practical inventory cap. Explicit hLP and auction execution bounds remain separate.
Mainnet startup validates profile-specific settings even when no eligible work exists.

Before signing, the journal quotes the exact transaction fee, simulates the final
message, and atomically reserves fees under a wallet lock. Instruction-enforced
maximum token debits are still recorded for auditing, without a daily budget check.
Successful and failed transactions consume the fee budget for the reservation's
UTC day. Unsubmitted/definitively absent transactions release their fee reservations;
unresolved fees remain charged across midnight, restarts and releases. Changing
the policy cannot erase recorded spending. A fee-budget failure stops signing.

Inventory liquidation deliberately has no swap or profit filter. The auction
worker likewise purchases protocol revenue with a bounded payment; it does not
guarantee an external resale or a net-profit floor. Keep auction and hLP services
in shadow until their specific economics, market configuration and execution
evidence have been reviewed. Do not infer profitable arbitrage from the profile name.

## Operator checks

1. Review the current revision's [release decision](evidence/mainnet-release-waiver.json)
   and run `REQUIRE_FROZEN_PROTOCOL=1 node scripts/validate-repo.mjs`. The complete
   matrix is waived, not passed. Later revisions require a new release decision.
2. Stop signing workers, then apply every PostgreSQL migration in filename order.
   Migration `0005` makes pending-position conflicts independent of revision.
   Reconcile duplicate pending generations before proceeding; do not run old and
   new lease implementations together during this upgrade.
3. With the intended RPC in `SOLANA_RPC_HTTP_URL`, run
   `dusk-keeper --check-mainnet-deployment`. It accepts a captured candidate and
   checks deployed binaries, IDLs and authorities without loading a signer.
4. Supply each service's fee policy, database configuration and distinct KMS
   identity or explicitly selected local key file. On the frozen release,
   `KEEPER_MODE=live dusk-keeper --check-execution` validates deployment, public
   signer identity, settings and journal schema without submitting. This does
   not itself prove KMS Sign permission or a successful mainnet transaction.
5. After an owner-created controlled market and reviewed execution policies are
   available, start one authorized signing worker with
   `KEEPER_MAX_SENDS_PER_PASS=1`. Inspect finalized receipts and durable fee
   reservations before activating other signing profiles.

Remote database connections verify TLS certificates and hostnames. A private
database CA may be supplied as a public PEM file via `DATABASE_CA_CERT`.
`DISCOVERY_INTERVAL_MS` defaults to 5000; `/readyz` degrades when the last completed
scan exceeds `KEEPER_MAX_SCAN_AGE_MS` (default 120000). Long but healthy scans may
need an explicitly larger threshold. `/healthz` remains a process-liveness check.
