# Devnet capture — 4 November 2026

The keeper targets the observed deployment, revision `devnet-2026-11-04-c7db901`.
Dusk `main` was pulled to `e309197`; its srLP rename is newer than the deployed
IDLs. A clean detached checkout of `c7db9013b9d751da5a734914e3efae203ccdf59b`
has exactly matching vendored IDLs and supplies SDK `2.16.0-next.20261102` plus
the fixture builders. The native keeper's srLP aliases preserve the deployed
yLP instruction fields and PDA seeds.

The capture reads the devnet genesis hash, upgradeable-loader Program and
ProgramData accounts, and Anchor IDL accounts. It checks their owners and
executable flags, compares the IDLs with source, and re-reads each deployment to
reject changes during capture. The full payload after the 45-byte ProgramData
header is saved, including padding. No wallet or signing key is used.

`protocol/devnet-deployment.v1.json` records the observed program IDs, account
addresses, upgrade slots, sizes and SHA-256 hashes. The validator cross-checks
this evidence against the lock and artifacts offline. It does not prove the
cluster has remained unchanged after capture. Matching IDLs and passing tests
also do not constitute a reproducible-build attestation for the deployed bytes.
The lock's toolchain describes the capture environment; Surfpool was unavailable
and is recorded as null instead of inheriting a historical version.

## Reproduce

Use Node 24, the pinned Rust toolchain, Anchor 0.31.1, Solana's `cargo build-sbf`,
and a clean source checkout with matching IDLs. Keep that checkout separate from
the latest Dusk `main`. Run `node scripts/prepare_build.js` there before any Anchor
commands, as required by Dusk's repository instructions. Install its locked
dependencies, prepare `target/idl` and `target/types` from its tracked SDK IDLs
and types, run `npm run check:dusk-sdk`, and pack `packages/dusk-sdk` with npm.

From the keeper repository:

```bash
node scripts/capture-devnet.mjs /path/to/matching-source /path/to/sdk.tgz devnet-YYYY-MM-DD-commit
```

This downloads the observed executable payloads into the source checkout's
ignored `target/deploy`, then pins the binaries, IDLs and SDK and regenerates the
instruction contracts, codec fixtures and account layouts. It leaves the lock
captured. `SOLANA_RPC_HTTP_URL` may override the public endpoint; a non-devnet
genesis hash is rejected.

Build only the test helpers in the source checkout:

```bash
cargo build-sbf --manifest-path tests/fixtures/referral_transfer_hook/Cargo.toml --sbf-out-dir target/deploy
cargo build-sbf --manifest-path tests/fixtures/token_metadata_fixture/Cargo.toml --sbf-out-dir target/deploy
```

Then regenerate the bank fixtures and validate from the keeper repository:

```bash
node scripts/capture-liquidation-fixtures.mjs /path/to/matching-source
node scripts/capture-hlp-fixtures.mjs /path/to/matching-source
node scripts/validate-repo.mjs
cargo build --locked
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
npm run typecheck
node --experimental-strip-types --test --test-concurrency=1 typescript/tests/*.test.ts
npm run keeper:ts:check
```

The fixture capture executes downloaded programs in LiteSVM. Native bank tests
cover ordinary and internal flash liquidation, emergency settlement, hLP
recovery, conditional orders and rollback. Oracle tests check all four supported
feed formats, zero-weight policies and native account parity; a normalized-feed
bank test verifies successful health preview and rejection of omitted, stale or
incorrectly owned reports. Runtime execution snapshots include active feeds so
report changes participate in revalidation before signing.

This update prepares ABI compatibility and shadow testing. A funded devnet wallet,
chosen test market, matching frontend cluster, explicit execution bounds and the
separate frozen-lock release procedure are still required for a signed end-to-end
test. It does not enable live mode or establish that the beta frontend currently
uses this deployment.

A read-only market scan on 4 November found one matching 6,101-byte Market,
`8Ar6nVbzp3c3o7ByHXnRfYMc4RNfbrUSjPMcz57CEhDD`, with a normalized oracle policy
and weight 5,000 bps. It also found the older 6,069-byte account
`GXwH8XtZKZre23d7Hp8VkbuGgmXMfKo4riVKbK5uj3gZ`, which fails the current layout
guard and is excluded from lending discovery. Recheck market and oracle state
when selecting a test market; this observation is not a migration or freshness
guarantee.
