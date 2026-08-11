# Keeper incident response

## Contain

1. Set affected services to `KEEPER_MODE=shadow` and confirm no new signer requests occur.
2. If key compromise is possible, disable the remote-signing policy and rotate the service-specific wallet.
3. Preserve Railway logs, database attempt rows, deployed image digest, and `protocol.lock.json` revision.
4. Reconcile every submitted signature directly against RPC; do not blindly retry unknown attempts.
5. Record the first affected slot, markets, profiles, reason codes, and financial exposure.

## Diagnose

Compare indexer observations with direct RPC snapshots and Rust/TypeScript shadow
outputs. Verify program IDs and all artifact hashes. Classify the issue as data
freshness, protocol drift, scheduling/lease failure, transaction construction,
signing policy, confirmation, or economic-bounds failure.

## Recover

Patch and reproduce against the captured fixture, add it to conformance tests, and
run Surfpool. Deploy shadow first. Restore signer permissions at the smallest
limits, then promote one profile. Publish a sanitized incident record without
credentials, raw transactions containing secrets, or private RPC URLs.

