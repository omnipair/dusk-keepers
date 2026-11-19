# Dusk Keepers

Keeper runtimes for Dusk, pinned to the observed Solana mainnet deployment and
verified with shared conformance fixtures in Rust and TypeScript.

The `mainnet-2026-11-19` capture matches the finalized production deployment
from `aa196e3423fd5f46b00630a4f161d91f95ef7b3d` and registry-published SDK
`3.0.1`. Both source verifications passed. Exact on-chain padded executables,
IDLs, authorities and upgrade slots are recorded in the deployment receipt.
The lock is frozen for local live preparation under the revision-specific
[release-test waiver](docs/evidence/mainnet-release-waiver.json). The full new
release matrix and mainnet canary remain incomplete. Hosted rollout is separate.

Rust implements lending, leveraged liquidation and orders, hLP recovery, revenue,
lifecycle and sentinel workers. TypeScript provides independent protocol codecs
and reference checks. Both preserve oracle suffixes and direct account checks.

The `mainnet-2026-11-19` decision records the owner's existing instruction to skip
release tests and their request to prepare the new market for local live execution.
Production signing still requires deployed-artifact verification, simulations
and durable journal/fee checks.
See [mainnet release preparation](docs/mainnet-release.md). Historical production
artifacts and receipts are preserved under `artifacts/releases/mainnet-2026-11-14`.

## What is here

- `protocol/`: language-neutral JSON Schemas and protocol compatibility rules.
- `protocol/keeper-instructions.v1.json`: mechanically generated critical-instruction contract pinned to both IDLs.
- `protocol/keeper-account-resolution.v1.json`: generated static-account and PDA seed manifest.
- `fixtures/`: golden inputs shared by the Rust and TypeScript test suites.
- `rust/`: native execution, fresh RPC evaluation and durable PostgreSQL submission/recovery.
- `typescript/`: the shadow/reference runtime and independent protocol codecs.
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

`protocol.lock.json` is `frozen` for `mainnet-2026-11-19` under the scoped
[release-test waiver](docs/evidence/mainnet-release-waiver.json). This enables
live startup after deployment, execution settings and journal checks pass;
it does not claim the complete release matrix or a production canary passed.
Later revisions follow the [protocol upgrade process](docs/runbooks/protocol-upgrade.md).

For local mainnet execution with private key files, use the
[local mainnet launcher](docs/runbooks/local-mainnet.md).

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

Live Rust workers also print a receipt after the journal records a finalized
transaction. It shows the SOL transaction fee (including priority fees), other
SOL movements such as rent, keeper token balances before/after, and actual net
tokens paid or received. Dusk liquidation events add the reward, swap input/output,
trading fees, repayment, owner surplus, and any written-off debt. Transfer, mint,
burn and account-balance lines identify each token mint and source/destination;
Jupiter routes include their recorded token transfers. Amounts use integer token
atoms and receipt decimals, never floating-point rounding or assumed USD prices.

Submitted transactions are labeled pending until finalized. Failed receipts show
the charged fee without presenting rolled-back swaps or rewards as received.
Missing receipt details are marked unavailable. Receipt formatting uses the
already-fetched journal evidence and adds no RPC requests or signing decisions.

Colors are automatic in a terminal. Set `KEEPER_LOG_COLOR=always` when forwarding
output through a terminal launcher, or `KEEPER_LOG_COLOR=never` for plain text;
`NO_COLOR` always disables colors. `KEEPER_LOG_POSITIONS=1` also colors position
health and prints progress while liquidation routes are being searched.
Detailed logs also identify every preview amount, program rejection, inventory
funding shortfall, and simulated wallet debit/collateral credit. For purchases
that pay directly from the keeper's inventory without a swap or profit filter,
see the [inventory settings](docs/runbooks/lending.md#economic-bounds).

For offline inspection, `dusk-keeper --receipt-log < receipt.json` accepts
`{ "signature": "...", "action": "...", "position": "...", "market": "...",
"receipt": <finalized getTransaction result with base64 transaction> }` from stdin.
Run from the runtime/repository directory so the pinned `artifacts/dusk.json` is
available. This only formats supplied evidence; it does not fetch, verify finality,
sign, or submit transactions.

## Mutable transfer-fee notice handling

See [the #287160 monitor and rollout runbook](docs/transfer-fee-notice.md). The
monitor watches epoch transitions and both market mints; final-notice markets
receive priority and changed fees invalidate pending attempts. The native
callers now use the pinned #45 flash/emergency interface. The deployed payloads
and upgrade slots are recorded in the flash migration runbook. The frozen release
lock and keeper service activation remain separate release steps.
