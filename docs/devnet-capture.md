# Devnet capture

The [protocol lock](../protocol.lock.json) records the current revision, source
commit and published SDK archive. The [artifact notes](../artifacts/README.md)
describe that release. Liquidation and hLP bank fixtures execute the captured
binaries; generated instruction contracts and account layouts use the pinned IDLs.

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
The lock records the capture toolchain. Sixteen native-plan RPC simulations were
recorded for the November 8 capture on an isolated offline Surfpool. These
[historical results](evidence/operator-surfpool-simulations.json)
do not validate later captures. The rehearsal
can be reproduced with an offline Surfpool on port 18899, using
`--block-production-mode transaction` so wall-clock ticks do not overwrite fixture clocks, and
`node --experimental-strip-types scripts/rehearse-surfpool.mjs`.
This is simulation evidence, not the full signed E2E matrix
required for a frozen live release.

## Recover a stale local runtime

`devnet ... account is missing` or `account identity changed` means a captured
Program, ProgramData or IDL account no longer matches the chain. A redeployment
can close the old ProgramData and IDL accounts while leaving the executable
Program account present. The error names the account and runtime revision;
execution remains blocked.

Pull the reviewed keeper capture for the current deployment and rebuild the
keeper. If the launcher uses a separate runtime directory, pulling the checkout
alone does not update it. With the workers stopped, install the rebuilt binary,
`protocol.lock.json`, `protocol/` and `artifacts/` as one matching bundle, and
refresh any launcher manifest that pins its binary hash. Preserve the wallets
and PostgreSQL journal.

Run `dusk-keeper --check` from the installed runtime directory with the same live
devnet environment used by the launcher. With `KEEPER_ALLOW_CAPTURED_DEVNET=1`,
this verifies the network and captured deployment before returning, without
loading a signer or starting workers. Check every enabled profile before
restarting. If the reviewed capture still differs from devnet, recapture and
validate it using the procedure below; do not bypass the deployment check.

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

The native runtime supports an explicitly authorized local devnet test exception
with `KEEPER_ALLOW_CAPTURED_DEVNET=1`; see the README for all-market discovery,
transaction limits and deployment checks. Default startup still requires the
frozen-lock release procedure for live execution. A funded wallet, matching
frontend cluster and eligible positions/orders remain necessary for execution
tests. This capture does not establish which deployment the beta frontend uses.

Discover smoke markets under the program ID in the current capture. Markets from
an older deployment are not targets for the replacement program. Re-fetch each
market's state and oracle policy before selecting it for a test.
