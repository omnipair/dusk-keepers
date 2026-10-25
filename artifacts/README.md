# Captured protocol artifacts

`protocol.lock.json` is the active authority: Dusk #45 at `cd6f1ec0`, status
**captured**. It pins the current Dusk/delegate binaries, IDLs, SDK archive and
IDL compatibility fingerprints. Validation rehashes these artifacts and computes
the compatibility fingerprints from their IDLs.

`protocol.snapshot.json`, old Git bundles, patches, dependency locks and generated
types describe the historical Local Snapshot 0 capture. They are retained for
history and are not the source or interface of this candidate. Current generated
interfaces are in the packed SDK and current instruction/layout manifests.

`dusk-devnet-current.so` matches the current `dusk.so` payload byte for byte.
The hash-named devnet binaries are historical payloads retained for reference.
The current Dusk and delegate payloads were compared against devnet program data;
the upgrade slots, payload hashes and allocation padding are recorded in
`docs/flash-liquidation-migration.md`.

Use `scripts/capture-protocol.mjs` for a candidate re-pin after rebuilding from a
clean checkout. Follow `docs/flash-liquidation-migration.md` for bank fixtures and
verification. Neither capture nor local execution freezes or deploys a program.
Program keypairs are excluded. Keeper activation still requires the frozen-lock
release gate even after deployed bytes have been verified.
