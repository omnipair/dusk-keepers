# Captured layout-15 protocol artifacts

The [protocol lock](../protocol.lock.json) pins the finalized devnet executable upgrade from [Dusk #98](https://github.com/omnipair/dusk/pull/98). Both executables include the allocation padding read from ProgramData. The IDLs and published SDK archive match the original layout-15 release byte for byte.

The [deployment capture](../protocol/devnet-deployment.v1.json) records the two upgrade slots, executable hashes, and IDL accounts. The liquidation and hLP bank fixtures run against those captured executables.

Run `npm run check` to verify artifacts and generated contracts. This lock is captured and permits shadow evaluation. The live gate requires a reviewed frozen lock.
