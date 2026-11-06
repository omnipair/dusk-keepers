# Devnet capture — 6 November 2026

The current revision is `devnet-2026-11-06-operators`, captured from the fresh
Dusk and leverage-delegate deployment, updated for the compatible Dusk upgrade at finalized slot 519241008. Source
`fda084dce13fdb99da0731d8c74861eefd7432fc` preserves the observed IDLs. Liquidation and hLP fixtures were regenerated from that exact source and execute the current captured binary. The SDK artifact is downloaded from npm as
`@omnipair/dusk-sdk@2.18.0-next.20261106`; its integrity and SHA-256 are recorded
in the protocol lock. Consumers install the published package normally.

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
The lock records the capture toolchain, including Surfpool 1.2.0. The original capture had sixteen
native-plan RPC simulations pass on an isolated offline Surfpool with the exact
captured binaries. The [recorded results](evidence/operator-surfpool-simulations.json)
can be reproduced with an offline Surfpool on port 18899 and
`node --experimental-strip-types scripts/rehearse-surfpool.mjs`.
This is simulation evidence, not the full signed E2E matrix
required for a frozen live release.

## Reproduce

Use Node 24, the pinned Rust toolchain, Anchor 0.31.1, Solana's `cargo build-sbf`,
and a clean source checkout with matching IDLs. Keep that checkout separate from
the latest Dusk `main`. Run `node scripts/prepare_build.js` there before any Anchor
commands, as required by Dusk's repository instructions. Install its locked
dependencies, prepare its generated interfaces and run `npm run check:dusk-sdk`. Download the
exact published SDK tarball from npm, verify its registry integrity and hash, and
pass that file to the capture command.

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

The fresh smoke market is `EdqUE8D7JuTrdSAy4kpynjvcaKugguuyjHzC6vFvw3HD`.
Re-fetch its state and oracle policy before selecting it for a test; this address
is a deployment reference, not a freshness guarantee.
