# Captured protocol artifacts

`protocol.lock.json` is the active authority: health-only liquidation from
[Dusk PR #59](https://github.com/omnipair/dusk/pull/59), source `9d6e5373`, status
**captured**. It pins the locally tested Dusk/delegate binaries, regenerated IDLs,
SDK 2.15.0-next.20261030 and IDL compatibility fingerprints. Validation rehashes
those artifacts and recomputes their compatibility fingerprints.

`protocol.snapshot.json`, old Git bundles, patches, dependency locks and generated
types describe the historical Local Snapshot 0 capture. They are retained for
history and are not the source or interface of this candidate. Current generated
interfaces are in the packed SDK and current instruction/layout manifests.

`dusk-devnet-current.so` and hash-named devnet binaries retain the previous
deployment record. They are historical payloads, not the new health-only
candidate in `dusk.so`. This PR does not deploy the new binaries. Historical
upgrade slots and allocation-padding checks remain in the migration guide.

Use `scripts/capture-protocol.mjs` for a candidate re-pin after rebuilding from a
clean checkout. Follow `docs/flash-liquidation-migration.md` for bank fixtures and
verification. Neither capture nor local execution freezes or deploys a program.
Program keypairs are excluded. Keeper activation still requires the frozen-lock
release gate even after deployed bytes have been verified.
