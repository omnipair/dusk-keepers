# Captured layout 17 protocol artifacts

The [protocol lock](../protocol.lock.json) pins the local build of
[Dusk PR 106](https://github.com/omnipair/dusk/pull/106), commit
`c24e143f0fd16fa72772fe0c78e149309cf47738`. Both generated IDLs/types and the
SDK `2.23.1-next.20261114` archive come from that checkout. Fresh liquidation
and hLP bank fixtures execute against these local binaries.

This is a source capture; no current deployment observation is included.
Previous named executable snapshots, source bundles, lockfile snapshots and
worktree.patch are historical evidence, not the current pin. Only the files
and hashes in protocol.lock.json authorize this captured revision. See
[layout 17 capture](../docs/layout17-capture.md) and the
[capture runbook](../docs/devnet-capture.md).

Run `npm run check` to verify the artifacts and generated contracts. The
captured lock permits offline/shadow evaluation. Live execution retains the
frozen release gate and actual deployment verification requirements.
