# Captured layout-15 protocol artifacts

[The protocol lock](../protocol.lock.json) pins the local layout-15 release candidate. Both SBPF v0 executables and both Anchor IDLs were captured from the signed Dusk source in the lock. The SDK archive is the corresponding local pack of `2.21.0-next.20261112`.

Use `npm run check` to verify the artifacts and the generated account, instruction and adapter contracts. The captured lock permits fixtures, Surfpool and shadow evaluation. The live gate still requires a finalized devnet capture and a reviewed frozen lock.

After deployment and SDK publication, follow the [capture runbook](../docs/devnet-capture.md) to record both deployed executables, the matching IDL accounts and the published SDK. The historical snapshots, Git bundles, generated types and hash-named binaries retain their original provenance.
