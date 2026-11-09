# Flash/emergency keeper migration to Dusk #45

## Current devnet capture — 2026-11-06

The active revision is `devnet-2026-11-06-operators`. The fresh deployment uses
SDK `2.18.0-next.20261106`, published through Dusk's npm release workflow. Both
executables and IDLs were captured from devnet; source
`d8c7f89534b734792ec32af6914e53c3efeb04a9` supplies fresh lending/hLP fixtures.
The layout uses srLP, operator authority and an optional manager. Core LP voting
and the separate insurance vault/draw/donation paths have been removed. Funded
principal losses pass through junior backing and then senior backing.

| Program | Program ID | Deployment slot |
| --- | --- | --- |
| Dusk | `B3vxDQmtKFxfh6hmfLRWKaHZR2w4UPw18XJCTbfrrBa2` | `519164140` |
| Leverage delegate | `B315E6hHTgDjHgCCPungLop2mkgv9SHkcBc9ZzALNxNY` | `519164379` |

See [the capture runbook](devnet-capture.md) and
[the evidence manifest](../protocol/devnet-deployment.v1.json) for provenance,
payload sizes and hashes. The lock stays **captured** and services keep their
existing shadow mode. Signed activation requires the separate frozen release.

## Service behavior

| Profile | Native Rust action |
| --- | --- |
| `lending` | Preview borrowing health once, try ordinary atomic fills, then permitted emergency settlement when no acceptable ordinary route exists. Modes: `ordinary`, `emergency`, `both` (default). |
| `leverage` | Preview current health, try ordinary leverage fills, then permitted emergency settlement. |
| `hlp-recovery` | Preview both market-wide hLP vaults, attempt an inventory-funded atomic flash repair, then use terminal close only when a full quote permits it. |

The standalone trigger, bidder, and settler profiles have been removed. Configure
`KEEPER_PROFILE=lending`; see [the lending runbook](runbooks/lending.md). Auction instructions
for these positions are gone; protocol revenue auctions remain a separate service.
Run signing services with distinct wallets when a frozen release is authorized.
Health monitoring is part of each execution worker; shadow mode uses a public identity and no signing key.

Liquidation incentives depend only on current health. No distress episode or
registration instruction exists. Each pass simulates `preview_liquidation_health`
with read-only accounts, then independently previews an atomic fill. Advancing
Unix time alone cannot increase either incentive. Healthy positions are rechecked
every pass; final-notice markets get priority. Health previews never produce signatures; lending/leverage submit execution.

Every decision starts with fresh RPC position, market, both mint accounts, Clock,
vault, hLP state and active oracle reports. Indexer data never authorizes execution. Previews use the
program's symmetric-EMA eligibility and partial/full permissions. The keeper starts
with bounded geometric sizes, measures complete-route execution profit, refines the
best interval, and exhaustively scans the final 64-atom window so integer-rounding
peaks are not hidden by the coarse probes. Failure of this bounded search is **not
proof** that a full close is allowed. Only an accepted full preview permits the full
plan. A probe exceeding the chain compute budget is logged and skipped;
invariant/ABI errors remain loud. Eligible positions with no acceptable adaptively
searched route are explicitly reported as unfilled.

## Settlement modes and bounds

- **`dusk` (default):** an atomic begin/settle pair with Dusk's internal settlement
  accounts. Simulated net debt-token profit must meet the operator's minimum;
  existing wallet collateral cannot be spent to hide a loss.
- **`inventory` (opt-in):** the buyer supplies debt tokens and keeps seized
  collateral. The payment is grossed up using the *effective* transfer fee.
  A single market, maximum gross wallet debit and minimum net collateral credit
  must be configured. This is an explicit inventory purchase, not an automatically
  profitable arbitrage or a Jupiter routing service.
- **Emergency:** uses #45's internal EMA price/smoothed-depth settlement, normal
  fees, authoritative reward and loss-waterfall quote. Same-position locks and
  the emergency gate remain program-enforced.
- **hLP recovery:** the keeper supplies the borrowed token from bounded inventory
  and receives the target token quoted by the program. It verifies the complete
  begin/payment/settle transaction, the closed session, an empty isolated payment
  vault, wallet debit/credit bounds and changed market state. A full terminal
  close has separate funded-loss and bounty limits.

The native and TypeScript builders also accept externally supplied route
instructions between begin and settle. The service currently implements the two
modes above; automatic Jupiter route discovery is not part of this migration.
Routes must finish in the same transaction, use the computed settle index and
satisfy the net payment. Owner payouts are owner-owned ATAs. Existing repayment
vault donations are preserved; the session closes but its token vault can remain.

The runtime simulates the complete transaction and checks debt-share reduction,
wallet deltas, released session and preserved payment-vault balance. It re-reads
and revalidates safety state before live signing; changed fee schedules or epochs
force a fresh quote. Submitted bytes are never rewritten to follow a new quote.

## Configuration

All token amounts below are integer raw atoms, not decimals or dollar values.
Use market-scoped workers when assets have different units/economics.

| Variable | Meaning |
| --- | --- |
| `KEEPER_MODE=shadow` | Simulate only; candidate locks cannot enter live mode. |
| `KEEPER_SHADOW_PUBLIC_KEY` | Public buyer identity; shadow does not load a private key. |
| `KEEPER_FLASH_SETTLEMENT=jupiter` | Default ordinary route; explicit `dusk` for internal settlement/devnet, or `inventory`. See [Jupiter routing](runbooks/jupiter-routing.md). |
| `KEEPER_FLASH_MAX_REPAY` | Positive maximum net debt repayment for ordinary fill probes. |
| `KEEPER_LIQUIDATION_MIN_PROFIT` | Minimum simulated debt-token gain for internal/emergency execution. Account for SOL fees operationally; there is no built-in SOL exchange-rate feed. |
| `KEEPER_EMERGENCY_SLIPPAGE_BPS` | Emergency reward/output quote tolerance, `0..10000` basis points (up to 100%); default `0`. The configured profit floor remains enforced on chain. |
| `KEEPER_FLASH_MARKET` | Required single-market public key for inventory purchases. |
| `KEEPER_FLASH_MAX_PAYMENT_DEBIT` | Required maximum gross wallet debit in inventory mode. |
| `KEEPER_FLASH_MIN_COLLATERAL_CREDIT` | Required minimum net seized collateral in inventory mode. |
| `KEEPER_LIQUIDATION_LOOKUP_TABLE` | Optional active, warmed-up LUT for transaction account compression. Needed when the plan exceeds the 1,232-byte packet limit. |
| `KEEPER_HLP_MARKET` | Required single market public key for an `hlp-recovery` worker. |
| `KEEPER_HLP_MAX_PAYMENT_DEBIT` | Maximum gross borrowed-token debit from the hLP keeper wallet. |
| `KEEPER_HLP_MIN_COLLATERAL_CREDIT` | Minimum net target-token credit for a flash repair. |
| `KEEPER_HLP_MAX_SOCIALIZED_LOSS` | Maximum principal loss the worker authorizes for a full hLP settlement. |
| `KEEPER_HLP_MIN_BOUNTY_OUT` | Minimum borrowed-token bounty for the terminal-close fallback. |
| `KEEPER_ORDER_MARKET` | Optional current market scope for conditional entry, TP/SL and hLP orders in the leverage worker. |
| `KEEPER_ORDER_MIN_BOUNTY_CREDIT` | Required minimum simulated net executor credit for conditional orders, in the payout token's raw atoms. Use a market-specific worker for different payout units. |

No default economic thresholds are guessed. Missing limits fail visibly.
Associated token accounts are prepared before the atomic pair when necessary.
Lookup tables are read from RPC and checked for ownership, warm-up and deactivation;
the compiled wire size is checked before signing. `--once` runs one scan for
local/shadow diagnostics without starting a permanent worker.

## Conditional orders

The native leverage worker discovers leverage entry orders, TP/SL close orders,
and hLP stop-loss/stop-rate orders from the pinned delegate program. Removed
markets are filtered before decoding their retired positions or orders. Trigger
prices, close fractions, entry limits, expiry and settlement requirements remain
enforced by the programs during full-transaction simulation. The worker does not
replace those checks with an indexer estimate.

Successful simulations must close entry/exit order accounts or transition an hLP
order out of its active state, and pay the configured net bounty. Partial TP/SL
closes use the order's stored fraction. hLP orders for a retired receipt cohort
are left for owner cancellation/yield settlement. Missing lookup-table capacity
or insufficient payout produces an explicit skip. No transaction is submitted in
shadow mode.

Before any authorized live signature, all instruction accounts are re-read,
mint/Clock fingerprints are revalidated, and the same durable submission journal
and position/side conflict lease used for liquidations is acquired. The captured
protocol lock still prohibits live execution; adding an executor does not grant
release authorization.

## Durable execution and restart

For an authorized frozen release, apply
`deploy/postgres/migrations/0002_flash_attempts.sql` and then
`deploy/postgres/migrations/0003_hlp_recovery_job.sql` to the keeper database
before starting the migrated workers. These are additive migrations with separate
`keeper_flash_work` and `keeper_flash_events` tables. Do not apply it to the Dusk
indexer database. Remote database connections require validated TLS.

1. Acquire a PostgreSQL session advisory lease for protocol revision/position/side.
2. Persist the work, fresh-state hash, unsigned message and blockhash validity.
3. Sign and persist exact transaction bytes plus `submitted_unknown` before RPC.
4. On restart, reconcile pending work even if the position no longer exists.
5. Re-broadcast only the saved bytes. A new generation requires finalized success,
   failure, or expired blockhash plus complete finalized history proving absence.
   A finalized receipt must contain the exact persisted transaction bytes; a
   signature-status response alone does not mark execution successful.

A null signature status alone never permits replacement. Missing archival history
keeps the work pending and surfaces an error. Database or lease failure stops
signing. Finalized transaction success and subsequent position scans are distinct:
a successful partial fill does not establish complete liquidation of the position.

## Local verification

```sh
npm ci
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked
node scripts/validate-repo.mjs
npm run typecheck
npm test
npm run keeper:ts:check
# Disposable local PostgreSQL only; tests create and drop their own test schema.
KEEPER_TEST_DATABASE_URL='postgresql://localhost/keeper_test' \
  cargo test --locked -p dusk-keeper flash_journal -- --ignored
```

The tests cover native/TypeScript byte and PDA parity; strict return decoding;
borrowing and leverage; both debt sides; current/pending capped transfer fees;
internal and inventory settlement; concentrated pools and active hLP; emergency
partial/full loss accounting; preserved repayment donations; hLP partial/full
bank execution and terminal close; underpayment and missing-settle rollback;
native RPC discovery/read-only-health/atomic-fill; and PostgreSQL
lease contention, pending-signature restart, and finalized-absence reconciliation.

`fixtures/litesvm` are disposable bank states captured from the exact protocol
commit. They contain public keys and token/account state, **no private keys**.
Signature verification is disabled only in local bank tests. To reproduce them:

```sh
node scripts/capture-liquidation-fixtures.mjs /path/to/exact-clean-dusk-checkout
node scripts/capture-hlp-fixtures.mjs /path/to/exact-clean-dusk-checkout
```

To capture a later candidate, first rebuild Dusk, delegate and packed SDK from
that clean checkout. Then use `scripts/capture-protocol.mjs` with checkout, SDK
archive and revision arguments, regenerate bank fixtures and rerun all checks.
The capture script never freezes or deploys. Local bank tests do not substitute
for the separately authorized release upgrade, deployed-binary attestation,
Surfpool/devnet rehearsal or live-release gate.
