# Railway deployment runbook

## Initial shadow deployment

1. Create one Railway service per entry in `deploy/railway/profiles.json`; do not combine wallets or job profiles.
2. Point Rust services at `deploy/railway/railway.rust.json` and TypeScript services at `deploy/railway/railway.typescript.json`.
3. Provision private Postgres and Redis, then configure RPC, WebSocket, indexer, allowlist, and non-secret tuning values.
4. Configure only a remote signer key identifier; never store a seed phrase or raw keypair JSON in Railway variables.
5. Keep `KEEPER_MODE=shadow` until the release checklist and shadow-parity window pass.

The sentinel is the only service with `signingRequired: false`; configure its
signer dependency as `disabled`. Lending trigger, bidder, and settler must use
three different signer policies even when the same operator runs all services.

Railway `/readyz` must eventually include RPC lag, WebSocket state, signer policy,
database/lease availability, protocol revision, last completed scan, and circuit
breaker state. The scaffold endpoint reports process readiness only and is not a
live-release health implementation.

## Live promotion

Run the CI `live_release` dispatch, deploy by immutable container digest, and start
with one instance and minimal signer limits. Confirm candidate discovery and
outcome records in shadow first. Enable live mode for one profile, verify at least
one expected skip and one successful Surfpool execution, then canary against the
chosen public cluster before considering mainnet.

## Required alerts

- RPC or indexer slot lag above threshold.
- No successful scan within two scan intervals.
- Retryable/terminal failure rate above threshold.
- Signer balance below reserve or policy denial.
- Lease contention, stuck in-flight attempt, or parity mismatch.
