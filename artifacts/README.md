# Captured protocol artifacts

`protocol.lock.json` pins the 4 November 2026 Solana devnet capture, revision
`devnet-2026-11-04-c7db901`, status **captured**. `dusk.so` and
`leverage_delegate.so` are the complete deployed ProgramData payloads after the
45-byte metadata header, including allocation padding. Their IDLs were fetched
from the programs' Anchor IDL accounts and match source `c7db901` exactly.
The SDK archive is `2.16.0-next.20261102`, built and tested from that source.

[Deployment evidence](../protocol/devnet-deployment.v1.json) records program IDs,
ProgramData and IDL addresses, observed and upgrade slots, sizes and hashes.
The repository validator checks this record against the lock and local artifacts.
This proves artifact consistency with the observation; matching IDLs do not
prove the deployed executables were built from that source. The lock's toolchain
records the capture environment, not an attested deployed build environment.

`protocol.snapshot.json`, old Git bundles, patches, dependency locks, generated
types, `dusk-devnet-current.so` and hash-named binaries are historical captures.
They are not the active interface. Current generated interfaces live in the
packed SDK and the instruction, account-resolution and layout manifests.

Use `scripts/capture-devnet.mjs` and the [capture runbook](../docs/devnet-capture.md)
to refresh from the chain. `scripts/capture-protocol.mjs` remains available for a
local source candidate and clears previous deployment evidence. Neither path
freezes the lock, signs transactions or deploys programs. Program keypairs are
excluded; activation still requires the frozen-lock release gate.
