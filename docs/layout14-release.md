# Fresh devnet protocol compatibility

The active capture is `devnet-2026-11-09-fresh-layout14`, from program source
`2b6311f4b4f8c1678edee72e8a3dbd1ff3f467b5`. Dusk and the delegate were finalized
at slots 520349136 and 520350149. Both fresh markets were created after the
complete revision boundary, 520350150. Previous program and IDL accounts were
archived and closed to reclaim rent.

The captured SDK is the registry-published `2.21.0-next.20261109` artifact, SHA-256
`645f872b2d786195f9153f5b784776159400ec7ca6d735d442875be70e9b7116`.
Keepers read the IDLs and use native Rust/TypeScript implementations. Contracts,
account layouts, PDA vectors and liquidation/hLP banks are regenerated for the
fresh program IDs. Market layout 14 and the junior-exit behavior are unchanged
from the combined release: cancel during notice, then request a replacement
with a full notice; capital remains junior until conversion.

Validation passed 215 TypeScript/SVM tests, 69 native Rust tests, the PostgreSQL
lease/signing-generation recovery test and all 16 offline Surfpool simulations
recorded in `docs/evidence/operator-surfpool-simulations.json`.

The lock remains captured. Hosted services retain their existing shadow mode;
this does not claim the complete frozen-lock live-release matrix or Jupiter
mainnet-fork acceptance. No signing guard is weakened. Devnet retains
`KEEPER_FLASH_SETTLEMENT=dusk`, since hosted Jupiter serves mainnet.
