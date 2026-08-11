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
