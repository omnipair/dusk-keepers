# Mainnet keeper preparation

The active revision is `mainnet-2026-11-14`, captured from the production
release at Dusk commit `0acc9057b53a0495571f1bbf8a18e5db0f2efd45` and the
published SDK `2.27.0`. Both programs are independently source verified.

| Program | ID | Deployed slot |
| --- | --- | ---: |
| Dusk | `DuskWhswJYrzABDn8GqQD191X9RobRi5jhgGntXEdnKj` | 469349657 |
| Delegate | `DeLegABhDLREbDpwwcsbkHvDS5MwqKPAw1CJKoooUXFQ` | 469350143 |

`protocol/mainnet-deployment.v1.json` records the observed upgradeable-loader
accounts, upgrade authorities, executable bytes and IDL account hashes. The
validator checks the receipt against the vendored artifacts. At live startup,
the native runtime also verifies the mainnet genesis, both deployed binaries,
IDLs and authorities; it rechecks deployment metadata each pass and around
submission. A change requires recapture and release review.

## Deployment state

The lock is **frozen** under the explicit owner instruction on 2026-11-16:
“skip the release tests, make it mainnet ready.” The scoped
[waiver receipt](evidence/mainnet-release-waiver.json) records the incomplete
release matrix without marking those scenarios passed. Verified artifact pins
and runtime deployment, simulation, fee/reserve and journal checks remain in force.
This waiver applies only to `mainnet-2026-11-14`.

The owner also selected local private-key files for the Mac launcher. Follow the
[local mainnet runbook](runbooks/local-mainnet.md); KMS remains supported for AWS.

The [Surfpool evidence](evidence/mainnet-surfpool-simulations.json) names the
exact executable, IDL and SDK hashes. Reproduce it with an isolated offline
Surfpool on port 18899, `--block-production-mode transaction`, and
`node --experimental-strip-types scripts/rehearse-surfpool.mjs`. Transaction
clock mode preserves each captured fixture clock. Clock mode can overwrite
fixture sysvars with wall time and invalidate time-dependent eligibility.

The [signed liquidation/hLP receipt](evidence/mainnet-signed-liquidations.json)
records 16 successful local submissions with forged-signature rejection and
flash-session closure checks against those production binaries. Its
`completeLiveReleaseMatrix` remains false; it does not cover the missing signed
order, auction, lifecycle and complete fault scenarios.

Source-history bundles and dependency locks describe the clean deployed source.
The previous devnet chain receipt is historical data at
`protocol/archive/devnet-2026-11-08-hlp-deposit/deployment.v1.json`; it is not
an active mainnet capture or authority to execute on devnet.

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
script creates or funds a market. No production canary is claimed by the waiver.

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

1. Build the binary and run `REQUIRE_FROZEN_PROTOCOL=1 node scripts/validate-repo.mjs`.
   The remaining release-test matrix is waived for this revision; ordinary CI
   continues to check the code. No test result should be fabricated for the waiver.
2. Stop signing workers, then apply every PostgreSQL migration in filename order.
   Migration `0005` makes pending-position conflicts independent of release revision.
   Duplicate pending generations cause it to fail and must be reconciled first.
   Do not run old and new lease implementations together during this upgrade.
3. With the intended RPC in `SOLANA_RPC_HTTP_URL`, run
   `dusk-keeper --check-mainnet-deployment`. This accepts a captured candidate and
   checks deployed binaries, IDLs and authorities without loading a signer.
4. Review the scoped [release waiver](evidence/mainnet-release-waiver.json). For
   any later revision, complete the normal [release process](runbooks/protocol-upgrade.md).
5. Supply each service's fee policy, database configuration and distinct KMS
   identity or explicitly selected local key file. On the frozen candidate, `KEEPER_MODE=live dusk-keeper --check-execution`
   checks deployment, public signer identity, profile settings and journal schema
   without signing or submitting. It does not prove KMS Sign IAM permission; the
   complete signing and submission path has not been exercised on mainnet.
6. Start one signing worker with `KEEPER_MAX_SENDS_PER_PASS=1`; inspect finalized
   receipts and journal reservations before enabling the other signing profiles.

Remote database connections verify TLS certificates and hostnames. A private
database CA may be supplied as a public PEM file via `DATABASE_CA_CERT`.
`DISCOVERY_INTERVAL_MS` defaults to 5000; `/readyz` degrades when the last completed
scan exceeds `KEEPER_MAX_SCAN_AGE_MS` (default 120000). Long but healthy scans may
need an explicitly larger threshold. `/healthz` remains a process-liveness check.
