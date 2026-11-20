# Surfpool keeper E2E matrix

Use the exact program binaries, IDLs, SDK, program IDs, and revision from the
frozen protocol lock. Seed both META/USDC market forms: constant-product and
concentrated liquidity.

| Scenario             | Required assertion                                                                                                                                 |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Leverage liquidation | Unhealthy position closes atomically; fees and collateral settle; repeat becomes `already_resolved`                                                |
| Lending liquidation  | Repay/seize bounds hold for both pool forms; competing worker loses lease or becomes an expected skip                                              |
| TP and SL            | Unmet trigger does not sign; met trigger closes once; liquidation supersedes a simultaneous order                                                  |
| Revenue auction      | Payment and sold-token bounds hold; inventory availability, concurrent bids and final settlement are checked; no resale-profit floor is assumed    |
| Lifecycle            | Announced operator bundle executes after notice; successor acceptance and revenue archival hold; unauthorized signer and stale epoch/revision fail |

Also cover Token-2022 transfer fees, expired blockhash, WebSocket disconnect with
poll recovery, restart during confirmation, stale oracle, RPC lag, database/Redis
outage, signer denial, and protocol-revision mismatch. Every case must compare
Rust and TypeScript candidates and instruction intents before asserting final
on-chain state.

## Signed captured-case rehearsal

A captured lock permits this partial offline rehearsal. It does not freeze the
release or replace the complete matrix above.

1. From the keeper repository, run `npm ci` and
   `cargo build --locked -p dusk-keeper`, then start a dedicated local instance with signature checks enabled:
   `surfpool start --offline --no-tui --no-deploy --no-studio --block-production-mode transaction --port 18899 --ws-port 18900`.
2. Run `node --experimental-strip-types scripts/rehearse-signed-surfpool.mjs`.
3. Review `/private/tmp/dusk-signed-keeper-proof.json`. Set
   `SURFPOOL_REHEARSAL_RPC` to another local HTTP RPC or
   `SURFPOOL_REHEARSAL_PROOF` to another output path when needed.

The script validates the pinned executables, rebinds public fixture buyer token
accounts to disposable in-memory signers, rejects a corrupted signature,
simulates with signature verification and submits each transaction to the local
validator. It checks committed execution and flash-session closure. No deployer
or KMS private key is read or exported. The receipt explicitly sets
`completeLiveReleaseMatrix: false`; TP/SL, auction, lifecycle and outage/restart
coverage still need their own evidence before the live-release gate passes.

## Signed protection-order rehearsal

With the same isolated signature-verifying Surfpool instance, run
`node --experimental-strip-types scripts/rehearse-signed-orders.mjs`.
`SURFPOOL_REHEARSAL_RPC` selects a local HTTP RPC and
`SURFPOOL_REHEARSAL_PROOF` selects the receipt path.

The four public layout-19 banks cover TP and SL at 50% and 100% closure.
Each uses a fresh in-memory executor, checks actual committed order/position
state and executor fee credit, rejects a forged signature, and verifies a
resolved order cannot execute or pay twice. These banks start after entry
and delegation; they do not test the user's order-placement flow.

This is additional partial evidence. Unmet-trigger no-sign behavior, races
with liquidation, auction/lifecycle execution, failure/restart coverage and
complete Rust/TypeScript intent parity remain part of the live-release matrix.
The receipt deliberately retains `completeLiveReleaseMatrix: false`.
