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
