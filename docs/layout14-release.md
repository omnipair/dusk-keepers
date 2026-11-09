# Layout-14 protocol compatibility

This capture pins the finalized Dusk/delegate programs at slots 520272736 and
520273794, with the generated IDLs and registry-published SDK 2.19.0-next.20261109.
The SDK archive SHA-256 is
`086ea70869c175e1fefcd5c062d693e92d699479dcc6602732ab0f857e14590a`.
The source fingerprint covers program source; SDK 2.20's checked builders do not
change either program ABI and are not a runtime dependency of the keepers.

Market layout 14 adds the independent utilization band. Account offsets,
instruction/account-resolution contracts and cross-language vectors are
regenerated from the IDLs. Liquidation and hLP banks are recaptured through the
program's instruction tests. Old layout-13 market accounts cannot be operated;
new discovery must use the new indexer revision and fresh supported markets.

Validation includes native Rust tests, 215 TypeScript/SVM tests, PostgreSQL
lease/signing-generation recovery, and 16 offline Surfpool liquidation/hLP
plan simulations recorded in `docs/evidence/operator-surfpool-simulations.json`.
The lock remains captured: these checks do not claim the complete live-release
matrix in `docs/runbooks/surfpool-e2e.md`, nor Jupiter mainnet-fork acceptance.
Keep hosted services non-signing until the frozen-lock and live-release gates
pass. Do not enable the local captured-devnet signing exception on hosted services.

Devnet uses `KEEPER_FLASH_SETTLEMENT=dusk`; hosted Jupiter serves mainnet.
This is deployment configuration, with no protocol or adapter bypass.
