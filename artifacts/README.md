# Frozen protocol artifacts

Local Snapshot 0 contains immutable copies of the two program binaries, IDLs,
generated types, packed Dusk SDK, source patch, source/integration Git bundles,
and dependency locks. Exact runtime artifact SHA-256 values are recorded in
`protocol.lock.json`; full capture provenance is in `protocol.snapshot.json`.

Do not hand-edit generated artifacts. Regeneration creates a new protocol revision
and must follow the protocol-upgrade runbook.

Program keypairs are intentionally excluded. A successful deployment must verify
the executable at the declared program ID and match its program-data hash to the
corresponding bundled `.so`.
