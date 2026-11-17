# Keeper PostgreSQL ledger

Apply migrations once, in filename order, with a deployment identity that is not
available to the keeper runtime. The application identity needs only normal CRUD
access to keeper tables; it must not own the schema or migration role.

`work_key` is the lowercase SHA-256 of the canonical candidate identity fields.
The primary and composite unique constraints make discovery idempotent. Event
append must run in a transaction that locks the attempt row and verifies both
`event_sequence` and `row_version`; stale writers retry from the persisted state.
Only one non-complete attempt may exist for a work key. A journaled
`retryable_failure` may create the next numbered attempt after backoff; it does not
reuse or overwrite the completed attempt.

Never delete or rewrite signing generations during ordinary operation. A
`submitted_unknown` generation must reach `confirmed` or `finalized_not_landed`
before inserting the next generation.

The Rust runtime uses `keeper_flash_work` and `keeper_flash_events` (migration
`0002`) for every signing profile. Apply `0004` for durable per-wallet fee
reservations and token payment records, and `0005` for pending-position conflicts
across protocol revisions.
Stop workers during this upgrade: old releases use revision-scoped advisory locks.
The new runtime reconciles old pending generations before replacing them and
rejects missing budget/conflict migrations on mainnet. Do not delete pending rows
to clear a spending cap or bypass an upgrade failure.

The daily fee budget uses the reservation's UTC date. Pending fees from earlier
dates remain charged. Failed transactions retain their fee reservation; abandoned
and definitively expired-absent work releases it. Ordinary restarts, policy changes,
and protocol revisions cannot reset these reservations. Token payment amounts are
recorded in the same table for auditing, with no transaction/day spending cap.
