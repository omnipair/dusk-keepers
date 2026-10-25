# Flash/emergency keeper migration to Dusk #45

**Candidate:** `pr45-938e3d1f`, source commit
`938e3d1fad25223852ac7c0a2ebed3b0203d04f8`. Lock status: `captured`.

This capture includes #52's `fund_interest` and hLP flash ABI and no longer
includes `rescue_hlp`. The separate `hlp-recovery` profile automates vault-wide
flash repair and terminal closeout. No deployment or production activation is
part of this change.

## Service behavior

| Profile | Native Rust action |
| --- | --- |
| `lending-trigger` | Observe a changed borrowing distress episode, including its recovery/reset. |
| `lending-bidder` | Observe when needed, preview ordinary borrowing fills and settle an atomic pair. |
| `lending-settler` | Observe when needed, preview permitted emergency partial/full borrowing settlement. |
| `leverage` | Observe, try ordinary leverage fills, then permitted emergency settlement. |
| `hlp-recovery` | Preview both market-wide hLP vaults, attempt an inventory-funded atomic flash repair, then use terminal close only when a full quote permits it. |

Existing profile names remain stable for service configuration. Auction instructions
for these positions are gone; protocol revenue auctions remain a separate service.
Run services with distinct signing wallets when a frozen release is authorized.

Observation is committed separately when the distress episode starts or resets.
A failed fill cannot roll back that previously committed start. Advancing
`observed_slot` alone does not trigger an endless observation loop. Healthy
positions are rechecked every pass; final-notice markets get priority.

Every decision starts with fresh RPC position, market, both mint accounts, Clock,
vault and hLP state. Indexer data never authorizes execution. Previews use the
program's symmetric-EMA eligibility and partial/full permissions. The keeper probes
bounded sizes; failure of those samples is **not proof** that a full close is
allowed. Only an accepted full preview permits the full plan. A probe exceeding
the chain compute budget is logged and skipped; invariant/ABI errors remain loud. Eligible positions
with no acceptable sampled route are explicitly reported as unfilled.

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

No default economic thresholds are guessed. Missing limits fail visibly.
Associated token accounts are prepared before the atomic pair when necessary.
Lookup tables are read from RPC and checked for ownership, warm-up and deactivation;
the compiled wire size is checked before signing. `--once` runs one scan for
local/shadow diagnostics without starting a permanent worker.

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
native RPC discovery/observation/fill; and PostgreSQL
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
