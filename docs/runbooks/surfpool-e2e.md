# Surfpool keeper E2E matrix

Use the exact program binaries, IDLs, SDK, program IDs, and revision from the
frozen protocol lock. Seed both META/USDC market forms: constant-product and
concentrated liquidity.

| Scenario | Required assertion |
| --- | --- |
| Leverage liquidation | Unhealthy position closes atomically; fees and collateral settle; repeat becomes `already_resolved` |
| Lending liquidation | Repay/seize bounds hold for both pool forms; competing worker loses lease or becomes an expected skip |
| TP and SL | Unmet trigger does not sign; met trigger closes once; liquidation supersedes a simultaneous order |
| Revenue auction | Bid executes only above net-profit floor; stale Jupiter quote and excess exposure are rejected |
| Lifecycle | Authorized proposal/activation/LP-lock action succeeds; unauthorized signer and stale revision fail |

Also cover Token-2022 transfer fees, expired blockhash, WebSocket disconnect with
poll recovery, restart during confirmation, stale oracle, RPC lag, database/Redis
outage, signer denial, and protocol-revision mismatch. Every case must compare
Rust and TypeScript candidates and instruction intents before asserting final
on-chain state.

