# Flash/emergency keeper migration to Dusk #45

## Current health-only candidate — 2026-10-30

[Dusk PR #59](https://github.com/omnipair/dusk/pull/59), commit `9d6e5373`, removes
liquidation episodes and registration. The active lock captures the new
programs, regenerated interfaces and SDK 2.15.0-next.20261030. All lending and
hLP bank fixtures are recaptured from that exact source and binary pair. The
position layouts change; adopt the matching protocol and keeper artifacts
as one candidate. This PR performs no deployment or live keeper activation.
The rollout records below describe the previous development deployment.


**Candidate:** `devnet-2026-10-26-cd6f1ec`, source commit
`77877d576018fd2bd8191d13f05534b0484e2282`. Lock status: `captured`.

This capture includes #52's `fund_interest` and hLP flash ABI and no longer
includes `rescue_hlp`. The separate `hlp-recovery` profile automates vault-wide
flash repair and terminal closeout. This pin follows the completed devnet program
upgrade; keeper services remain in shadow mode pending the frozen release gate.

## Verified devnet deployment

The upgradeable-loader metadata and downloaded program payloads match this capture:

| Program | Program ID | Upgrade slot | Payload SHA-256 |
| --- | --- | --- | --- |
| Dusk | `JA8Zxxm4t4zopBL8e3dQQXWfQ3a5pBUPY9Sp9RnybV2X` | `515502278` | `21e3f3c4dd3e0e5b4fad85f69e0bae702b1209ab4fe1820f6bc86e81408d07ed` |
| Leverage delegate | `AXNfmZt5e1UM4daeTzW3H7zNo4boobBcnFm8RzJYxvAv` | `515117216` | `4c1f7e1b326ff91d34d87edc888dee38bfde44058be50df3bb7ecdbe11845e1f` |

Dusk's 3,519,968 payload bytes match `artifacts/dusk.so`; its program-data
allocation has another 8,824 bytes, verified to be zero. The delegate's 1,149,896
payload bytes match `artifacts/leverage_delegate.so` with no trailing padding.
Both IDLs, the SDK archive and all five IDL compatibility fingerprints are
unchanged from `bc09061b`. The SDK remains `2.14.2-devnet.20261025`, SHA-256
`186c3d85c5a733da446c6f00301fefd0f36170336f1d8f4fdc9807ffc8c16d8c`.

This revision includes the position-principal/share-burn and frozen hLP yield
accounting fixes. Bank fixtures were recaptured from the exact source commit and
execute the deployed payloads locally.

The 27 October compatible repair sequence bounds emergency recovery proofs,
restores concentrated reseeding after the final external yLP exit, and permits
drained-market read previews. Only the
Dusk executable changed. The delegate remains the previously deployed
`cd6f1ec0` artifact, and its source/interface was not modified by the repair.
The event revision and SDK archive remain unchanged. All lending and hLP bank
fixtures were freshly captured and executed against the new Dusk binary;
provenance was not applied to banks captured from the previous binary.

## Service behavior

| Profile | Native Rust action |
| --- | --- |
| `lending-trigger` | Read-only current-health monitoring; never signs a registration transaction. |
| `lending-bidder` | Preview current health and ordinary borrowing fills and settle an atomic pair. |
| `lending-settler` | Preview current health and permitted emergency partial/full borrowing settlement. |
| `leverage` | Preview current health, try ordinary leverage fills, then permitted emergency settlement. |
| `hlp-recovery` | Preview both market-wide hLP vaults, attempt an inventory-funded atomic flash repair, then use terminal close only when a full quote permits it. |

Existing profile names remain stable for service configuration. Auction instructions
for these positions are gone; protocol revenue auctions remain a separate service.
Run signing services with distinct wallets when a frozen release is authorized.
The lending-trigger monitor uses a public preview identity and no signing key.

Liquidation incentives depend only on current health. No distress episode or
registration instruction exists. Each pass simulates `preview_liquidation_health`
with read-only accounts, then independently previews an atomic fill. Advancing
Unix time alone cannot increase either incentive. Healthy positions are rechecked
every pass; final-notice markets get priority. The monitor-only `lending-trigger`
profile never signs a health preview; bidder/settler/leverage submit execution.

Every decision starts with fresh RPC position, market, both mint accounts, Clock,
vault and hLP state. Indexer data never authorizes execution. Previews use the
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
  close has separate insurance, socialized-loss and bounty limits.

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
| `KEEPER_FLASH_SETTLEMENT=dusk` | `dusk` or explicitly opted-in `inventory`. |
| `KEEPER_FLASH_MAX_REPAY` | Positive maximum net debt repayment for ordinary fill probes. |
| `KEEPER_LIQUIDATION_MIN_PROFIT` | Minimum simulated debt-token gain for internal/emergency execution. Account for SOL fees operationally; there is no built-in SOL exchange-rate feed. |
| `KEEPER_FLASH_MARKET` | Required single-market public key for inventory purchases. |
| `KEEPER_FLASH_MAX_PAYMENT_DEBIT` | Required maximum gross wallet debit in inventory mode. |
| `KEEPER_FLASH_MIN_COLLATERAL_CREDIT` | Required minimum net seized collateral in inventory mode. |
| `KEEPER_LIQUIDATION_LOOKUP_TABLE` | Optional active, warmed-up LUT for transaction account compression. Needed when the plan exceeds the 1,232-byte packet limit. |
| `KEEPER_HLP_MARKET` | Required single market public key for an `hlp-recovery` worker. |
| `KEEPER_HLP_MAX_PAYMENT_DEBIT` | Maximum gross borrowed-token debit from the hLP keeper wallet. |
| `KEEPER_HLP_MIN_COLLATERAL_CREDIT` | Minimum net target-token credit for a flash repair. |
| `KEEPER_HLP_MAX_INSURANCE_DRAW` | Maximum insurance draw the worker authorizes for a full hLP settlement. |
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
