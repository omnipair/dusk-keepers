# Dusk Keepers

Keeper runtimes for Dusk, pinned to the observed Solana devnet deployment and
verified with shared conformance fixtures in Rust and TypeScript.

The current capture is `devnet-2026-11-06-operators`. The compatible Dusk upgrade at finalized slot 519241008 was recaptured from chain; the delegate and both IDLs are unchanged. The lock pins source
`fda084dce13fdb99da0731d8c74861eefd7432fc`. Liquidation and hLP bank fixtures were regenerated from that source and tested against the current captured binary.
The SDK artifact is the registry-published `@omnipair/dusk-sdk@2.18.0-next.20261106`;
it records provenance and is not a bundled runtime dependency. The active ABI uses
srLP and operator/optional-manager authority, with no core LP voting or separate
insurance fund.
Rust implements lending, leveraged liquidation and orders, hLP recovery, revenue,
lifecycle and sentinel workers. TypeScript provides independent protocol codecs
and shadow reference checks. Active oracle policies add the required read-only
reports to liquidation, hLP and order instructions and fresh execution snapshots.

The lock remains **captured** and defaults to **shadow mode**. Local bank tests
execute the downloaded devnet binaries. Live signing still requires the separate
frozen-lock release gate. See [the devnet capture record](docs/devnet-capture.md)
and [the flash migration runbook](docs/flash-liquidation-migration.md).

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

## Worker layout

The Rust application keeps each service in its own folder:

```text
rust/apps/keeper/src/
  workers/
    lending/      Borrowing discovery and ordinary/emergency policy
    leverage/     Leveraged liquidation and conditional orders
    hlp/          Aggregate vault recovery and its transaction plans
    revenue/      Protocol revenue auctions
    lifecycle/    Announced operator bundle execution
    sentinel/     Read-only discovery and transfer-fee monitoring
  liquidation/    Shared lending/leverage engine, plans, search and simulation
  runtime/        RPC, accounts, signing, transaction encoding and journal
  config.rs       Service profile and mode validation
  health.rs       Observation state and health/readiness endpoints
  main.rs         Startup and polling orchestration
```

`workers/mod.rs` dispatches a validated profile. Lending and leverage share the
liquidation engine so their freshness, economic bounds and durable-submission
checks stay aligned. hLP recovery owns its separate vault algorithm. TypeScript
keeps its independent protocol/reference libraries; it does not have live workers.

## Lending worker

`KEEPER_PROFILE=lending` combines borrowing discovery, health checks, ordinary
flash liquidation and emergency settlement. `KEEPER_LENDING_MODE=both` tries an
ordinary route first; `ordinary` and `emergency` restrict the worker to one
mechanism. Only an unavailable or below-bound route permits fallback; errors
stop evaluation, and unknown submitted signatures stay in journal recovery.
The former trigger, bidder and settler service names are rejected. Use shadow
mode for monitoring without signing. See [the lending runbook](docs/runbooks/lending.md).

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
callers now use the pinned #45 flash/emergency interface. The deployed payloads
and upgrade slots are recorded in the flash migration runbook. The frozen release
lock and keeper service activation remain separate release steps.
