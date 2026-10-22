# Dusk Keepers

Production-oriented keeper runtimes for Dusk, pinned to one protocol snapshot and
verified with the same conformance fixtures in Rust and TypeScript.

The candidate interface is Dusk PR #45 at `1bce17ba`. Rust implements borrowing
and leverage observation, atomic flash fills and emergency settlement. TypeScript
provides independent plan/quote codecs and a shadow reference runtime.

The captured program and SDK also contain the hLP flash interface merged from
PR #52. This keeper does not yet automate hLP flash recovery or terminal closeout;
its execution profiles remain borrowing and leverage. The lock stays captured
until the deployed program and release artifacts are verified separately.

The lock remains **captured** and defaults to **shadow mode**. Local tests execute
the pinned program binaries; they do not attest a deployed program. Nothing in
this migration deploys, freezes the lock, or enables signing on that candidate.
See [the flash migration runbook](docs/flash-liquidation-migration.md).

## What is here

- `protocol/`: language-neutral JSON Schemas and protocol compatibility rules.
- `protocol/keeper-instructions.v1.json`: mechanically generated critical-instruction contract pinned to both IDLs.
- `protocol/keeper-account-resolution.v1.json`: generated static-account and PDA seed manifest.
- `fixtures/`: golden inputs shared by the Rust and TypeScript test suites.
- `rust/`: native execution, fresh RPC evaluation and durable PostgreSQL submission/recovery.
- `typescript/`: the shadow/reference runtime and future lifecycle workers.
- `deploy/railway/`: separate Railway service profiles and hardened container builds.
- `docs/`: architecture, threat model, upgrade process, and operational runbooks.

The colleague-built `leverage-execution-bot` informed priority, deduplication,
race classification, and postcondition behavior. It is not copied or treated as
the canonical protocol client.

## Start here

```bash
npm ci
node scripts/validate-repo.mjs
cargo build --locked
cargo test --locked --workspace
npm run typecheck
npm test
npm run keeper:ts:check
```

Node tests invoke the native planner and execute its transactions in LiteSVM;
build the Rust binary first. Recovery tests also use a disposable PostgreSQL 16
instance (see the runbook). No public validator or funded wallet is needed.

Instruction contract drift is fail-fast. Regenerate only after intentionally
changing the pinned protocol snapshot:

```bash
node scripts/generate-instruction-contract.mjs --write
node scripts/generate-adapter-codecs.mjs --write
node scripts/generate-account-layout.mjs --write
```

## Safety state

`protocol.lock.json` is `captured`: all program, IDL, SDK, and compatibility
hashes are real and validated, while both adapters still reject live mode. Move
it to `frozen` only through
[`docs/runbooks/protocol-upgrade.md`](docs/runbooks/protocol-upgrade.md) after the
full Surfpool and cross-runtime gates pass.

## Mutable transfer-fee notice handling

See [the #287160 monitor and rollout runbook](docs/transfer-fee-notice.md). The
monitor watches epoch transitions and both market mints; final-notice markets
receive priority and changed fees invalidate pending attempts. The native
callers now use the pinned #45 flash/emergency interface. Deployment attestation,
the frozen release lock, and post-upgrade verification remain release steps.
