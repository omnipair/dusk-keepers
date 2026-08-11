# Protocol snapshot and upgrade runbook

## Freeze Local Snapshot 0

1. Export the current Dusk worktree into an isolated, read-only snapshot without committing or modifying the active working tree.
2. Build Dusk and leverage-delegate programs plus the packed TypeScript SDK with recorded toolchain versions.
3. Copy binaries, IDLs, generated types, SDK tarball, source patch, Git bundles,
   and dependency locks into an immutable artifact release; compute SHA-256 values.
4. Generate canonical fingerprints for instructions, accounts, events, errors, and PDA recipes; fill `protocol.lock.json`.
5. Mark the lock `captured` after artifacts and generated interfaces are
   hash-verified. Run SDK tests, both keeper conformance suites, and the full
   Surfpool matrix before changing the lock to `frozen`.

Two reviewers should verify that artifact hashes came from the named worktree
fingerprint. A commit SHA alone is insufficient when the source snapshot includes
uncommitted changes.

## Reconcile a later Dusk revision

Create a new lock revision; never mutate a frozen revision in place. Produce a
semantic diff covering instruction accounts/arguments, discriminators and account
sizes, events, errors, PDA seeds, arithmetic behavior, and authorization. Update
golden account and instruction vectors, then make each native adapter pass them.

Run both runtimes in shadow against the same Surfpool deployment. Require candidate
set, priority, calculated bounds, and instruction-intent parity before canarying
one Rust service. Promote one job profile at a time.

## Rollback

Set affected services to `KEEPER_MODE=shadow`, preserve attempt reconciliation,
and redeploy the last frozen lock plus matching container digest. Never pair an old
adapter with new program deployments. Revoke a signer only after recording any
submitted-but-unconfirmed attempts for continued read-only reconciliation.
