# Captured protocol artifacts

`protocol.lock.json` is the active authority: Dusk #45 at `5a9abcb3`, status
**captured**. It pins the current Dusk/delegate binaries, IDLs, SDK archive and
IDL compatibility fingerprints. Validation rehashes these artifacts and computes
the compatibility fingerprints from their IDLs.

`protocol.snapshot.json`, old Git bundles, patches, dependency locks and generated
types describe the historical Local Snapshot 0 capture. They are retained for
history and are not the source or interface of this candidate. Current generated
interfaces are in the packed SDK and current instruction/layout manifests.

Use `scripts/capture-protocol.mjs` for a candidate re-pin after rebuilding from a
clean checkout. Follow `docs/flash-liquidation-migration.md` for bank fixtures and
verification. Neither capture nor local execution freezes or deploys a program.
Program keypairs are excluded. Release requires separate deployed-byte attestation.
