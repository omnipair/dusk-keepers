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

The lock intentionally remains **captured**. The mainnet binding, artifact
recapture, native conformance suites, local PostgreSQL journal recovery and
16 offline Surfpool RPC simulations are preparation evidence. They do not
replace the full signed E2E matrix in `docs/runbooks/surfpool-e2e.md`, so live
promotion remains blocked by the existing frozen-lock gate.

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

Before live promotion, complete the frozen release checks, verified database
TLS, durable schema migrations and recovery, signing policies, wallet funding
and explicit market/asset spending limits. The owner creates the first market
through the UI; coordinate the subsequent controlled canary with that action.
Neither this release nor the KMS provisioning script creates or funds a market.

## Spending configuration

Start from [`deploy/mainnet/env.example`](../deploy/mainnet/env.example) and a
reviewed copy of [`risk-policy.example.json`](../deploy/mainnet/risk-policy.example.json).
The examples stay in shadow and are not automatically loaded. These are initial
canary limits **per signing wallet**, not a protocol-wide budget:

| Asset or expense | Per transaction | Per UTC day |
| --- | ---: | ---: |
| USDC inventory payment | 25 USDC | 100 USDC |
| WSOL inventory payment | 0.1 SOL | 0.4 SOL |
| SOL transaction fees, including priority | 0.0002 SOL | 0.02 SOL |

Each wallet retains at least 0.05 SOL after simulated rent/account creation and
fees. Rent is not part of the daily fee total. The reserve check conservatively
subtracts the quoted fee from the simulated payer balance even if the RPC already
deducted it. Fund inventory separately from this SOL reserve. Zero-bounty orders
remain supported; their fees still consume the daily fee allowance.

Amounts in the policy are decimal strings of raw atoms. Inventory purchases work
across all supported markets, capped by the debt/payment mint. An unlisted mint
has no inventory spending allowance; it does not prevent fee-only actions such as
an eligible emergency liquidation. Legacy global payment limits are additionally
clamped by the mint's policy. Mainnet startup validates profile-specific bounds,
even when no eligible work exists yet.

Before signing, the journal quotes the exact transaction fee, simulates the final
message, and atomically reserves fees and instruction-enforced maximum token
debits under a wallet lock. Successful payments retain their maximum reservation
for the reservation's UTC day. Failed transactions retain fee charges but release
token exposure. Unsubmitted/definitively absent transactions release both;
unresolved generations remain charged across midnight, restarts and releases.
Changing the policy cannot erase recorded spending. A budget failure stops signing.

Inventory liquidation deliberately has no swap or profit filter. The auction
worker likewise purchases protocol revenue with a bounded payment; it does not
guarantee an external resale or a net-profit floor. Keep auction and hLP services
in shadow until their specific economics, market configuration and execution
evidence have been reviewed. Do not infer profitable arbitrage from the profile name.

## Operator checks

1. Build and run the native/TypeScript suites. Run **all** ignored Rust database
   tests against a disposable database; CI covers both recovery and wallet budgets.
2. Stop signing workers, then apply every PostgreSQL migration in filename order.
   Migration `0005` makes pending-position conflicts independent of release revision.
   Duplicate pending generations cause it to fail and must be reconciled first.
   Do not run old and new lease implementations together during this upgrade.
3. With the intended RPC in `SOLANA_RPC_HTTP_URL`, run
   `dusk-keeper --check-mainnet-deployment`. This accepts a captured candidate and
   checks deployed binaries, IDLs and authorities without loading a signer.
4. Finish the [full release matrix](runbooks/surfpool-e2e.md), review the evidence,
   freeze the lock and pass the explicit CI live-release gate. Neither the deployment
   comparison nor a successful partial rehearsal substitutes for this gate.
5. Supply each service's reviewed spending policy, database configuration and KMS
   identity. On the frozen candidate, `KEEPER_MODE=live dusk-keeper --check-execution`
   checks deployment, public signer identity, profile settings and journal schema
   without signing or submitting. It does not prove KMS Sign IAM permission; the
   controlled canary still verifies the complete signing and submission path.
6. Start one signing worker with `KEEPER_MAX_SENDS_PER_PASS=1`; inspect finalized
   receipts and journal reservations before enabling the other signing profiles.

Remote database connections verify TLS certificates and hostnames. A private
database CA may be supplied as a public PEM file via `DATABASE_CA_CERT`.
`DISCOVERY_INTERVAL_MS` defaults to 5000; `/readyz` degrades when the last completed
scan exceeds `KEEPER_MAX_SCAN_AGE_MS` (default 120000). Long but healthy scans may
need an explicitly larger threshold. `/healthz` remains a process-liveness check.
