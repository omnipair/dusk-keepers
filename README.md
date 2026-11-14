# Dusk Keepers

Keeper runtimes for Dusk, pinned to the observed Solana devnet deployment and
verified with shared conformance fixtures in Rust and TypeScript.

The current capture is `devnet-2026-11-14-main-grid`, using the fresh layout-16
Dusk and delegate programs deployed at finalized slots 521965404 and 521967168.
The lock pins source `474ddcf4fcaecd1ed055238f1105a0a1323d7313` and both downloaded
executables and IDLs. Liquidation and hLP bank fixtures were regenerated from that
source and tested against the captured binaries.
The SDK artifact is the registry-published `@omnipair/dusk-sdk@2.24.1-next.20261114`;
it records provenance and is not a bundled runtime dependency. The active ABI uses
srLP and operator/optional-manager authority, with no core LP voting or separate
insurance fund.
Rust implements lending, leveraged liquidation and orders, hLP recovery, revenue,
lifecycle and sentinel workers. TypeScript provides independent protocol codecs
and shadow reference checks. Active oracle policies add the required read-only
reports to liquidation, hLP and order instructions and fresh execution snapshots.

The lock remains **captured** and defaults to **shadow mode**. Local bank tests
execute the downloaded devnet binaries. Live signing normally requires the separate
frozen-lock release gate. An explicitly enabled native devnet test verifies the
network and captured deployment, discovers all active supported markets and retains
durable journaling and transaction limits. See [the devnet capture record](docs/devnet-capture.md)
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
hashes are real and validated. Normal live startup still rejects this lock; the
native runtime has the explicit devnet-only test exception described below. Move
it to `frozen` only through
[`docs/runbooks/protocol-upgrade.md`](docs/runbooks/protocol-upgrade.md) after the
full Surfpool and cross-runtime gates pass.

For authorized local devnet testing, set `KEEPER_MODE=live`,
`KEEPER_ALLOW_CAPTURED_DEVNET=1` and `KEEPER_MAX_SENDS_PER_PASS=1`.
Execution covers all active supported markets; `KEEPER_DEVNET_MARKET` is no longer
used. Leave the optional `KEEPER_ORDER_MARKET` unset to cover orders in all markets.
Provide `SOLANA_RPC_HTTP_URL`, `DATABASE_URL`,
`KEEPER_SIGNER_KEY_PATH` and the profile's spending bounds. Auction testing also
requires `KEEPER_AUCTION_MAX_PAYMENT` in raw payment-token units. Startup hashes
the deployed programs and IDL accounts against the capture, and every pass and
broadcast rechecks the genesis and upgrade metadata. Deployment drift stops
execution until it is reviewed and recaptured. The sentinel remains wallet-free.

For one-off wallet preparation, additionally set `KEEPER_PREPARE_MARKET` to the
market whose token accounts you need. `dusk-keeper --prepare-devnet-wallet` creates missing
standard SPL base/quote token accounts through simulation, the durable journal
and the configured signer. It preserves the SOL reserve; the journal records
pending and finalized outcomes. This is wallet preparation, not a liquidation or
order-execution test. The lock stays `captured`, and this exception provides no
release authorization for other networks.

Set `KEEPER_LOG_POSITIONS=1` for detailed lending and leverage terminal output on
every pass. It prints position/owner/market addresses, debt and collateral in
their token units, raw debt, debt shares, mint addresses, snapshot/preview slots,
LTV, maintenance, health factor and the execution decision. Debt and collateral
value come from the program's health preview; collateral value is denominated
in the debt token, not USD. The displayed health factor is
`collateral_value × (1 − maintenance_bps / 10000) / debt`; liquidation is eligible
at or below one. Displayed ratios are truncated, while eligibility uses the exact
integer check. Debt-free positions have no health ratio.

Healthy positions remain visible. Supported positions evaluated after the pass's transaction budget is consumed
receive health previews marked `MONITOR ONLY`; diagnostics never select a route
or submit a transaction. Missing/unsupported markets and failed previews report
why health is unavailable. Logging does not change eligibility or execution limits.

## Mutable transfer-fee notice handling

See [the #287160 monitor and rollout runbook](docs/transfer-fee-notice.md). The
monitor watches epoch transitions and both market mints; final-notice markets
receive priority and changed fees invalidate pending attempts. The native
callers now use the pinned #45 flash/emergency interface. The deployed payloads
and upgrade slots are recorded in the flash migration runbook. The frozen release
lock and keeper service activation remain separate release steps.
