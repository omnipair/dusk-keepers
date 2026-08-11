# Execution lifecycle

The shared reducer accepts durable facts in this order:

`discover → revalidate → evaluate → policy → simulate → lease → sign → submit → confirm → postcondition → journal`

Indexer discovery is advisory. Revalidation reads direct RPC state and recomputes
the expected-state hash. Evaluation determines eligibility, while policy applies
operator limits such as allowlists, freshness, exposure, and minimum profit.
Simulation cannot substitute for either step.

## Expected races

Account changes/closures, already-closed positions or orders, triggers becoming
unmet, obligations becoming healthy, auctions settling, lease contention, and a
duplicate signature already confirming are typed `skipped` outcomes. They do not
increment infrastructure/program failure alerts. The exact race code remains in
the outcome for rate monitoring.

## Blockhash and re-sign rules

- **Expired before submit:** invalidate the signature, increment the signing generation, and restart at direct-RPC revalidation.
- **Submitted but result unknown:** retain the signature and enter reconciliation. Do not re-sign while it could still land.
- **Landed during reconciliation:** continue to postcondition with the original generation.
- **Finalized as not landed:** increment the generation, clear signing metadata, and restart at revalidation.

PostgreSQL records every generation and signature. The `(attempt_id, generation)`
primary key and unique signature prevent replacement from erasing evidence of the
first transaction.

