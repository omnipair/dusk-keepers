# Jupiter ordinary liquidation

The Rust lending and leverage workers default to `KEEPER_FLASH_SETTLEMENT=jupiter`.
The keeper uses a small typed Rust client for Jupiter's current
[Swap API V2 build endpoint](https://developers.jup.ag/docs/api-reference/swap/build).
There is no TypeScript sidecar or managed order/execute submission. Emergency
liquidation remains a separate Dusk operation. Explicit `dusk` and `inventory`
ordinary settlement modes remain available.

## Transaction and economics

One v0/legacy transaction contains keeper compute limits and validated ATA setup,
Dusk begin, Jupiter exact-in swap, a checked debt-token payment, and Dusk external
settle. The swap spends precisely the previewed collateral credit. Its output
goes to the keeper's debt ATA; only the fee-adjusted required payment moves into
the isolated repayment vault. The settle index is calculated after composition.
WSOL stays wrapped and cleanup/unwrap instructions are rejected.

`KEEPER_FLASH_MAX_REPAY` and `KEEPER_LIQUIDATION_MIN_PROFIT` are required in raw
debt-token units. The program-encoded minimum swap output, conservatively reduced
by any output transfer fee, must cover the gross transfer needed to credit Dusk's
net payment plus minimum profit. An API quote's displayed threshold alone is
insufficient. Both fee schedules and Clock participate in freshness checks.
External Dusk settlement requires `min_buyer_credit=0`; the Jupiter threshold and
fixed payment enforce this route's profit floor instead.

The whole transaction is simulated. The position must lose debt shares, the flash
session must close, existing repayment-vault donations must remain, debt balance
must increase by at least the profit floor, and existing collateral inventory
must not decrease. A route cannot consume funded intermediate keeper token
accounts. SOL spending (transaction fees and ATA rent) is checked separately;
the debt-token profit floor does not claim to include SOL gas costs.

## Configuration

| Setting | Default / limit |
| --- | --- |
| `JUPITER_API_KEY` | Optional secret header; provision through the secret store. |
| `KEEPER_JUPITER_SLIPPAGE_BPS` | `50`; `0..500`. Never dynamically widened. |
| `KEEPER_JUPITER_MAX_ACCOUNTS` | `32`; `8..64`. Reduce by 8 when the composite does not fit. |
| `KEEPER_JUPITER_MAX_BUILDS` | `4`; `1..16` HTTP requests per position/debt-side attempt, including size retries. |
| `KEEPER_JUPITER_MAX_BUILDS_PER_PASS` | `12`; `1..64` across the worker pass. |
| `KEEPER_JUPITER_REQUEST_INTERVAL_MS` | `2100`; `100..10000`. Shared process limiter; coordinate quotas across instances. |
| `KEEPER_JUPITER_TIMEOUT_MS` | `5000`; `100..10000` for each request. |
| `KEEPER_JUPITER_MAX_AGE_MS` | `10000`; `1000..30000` from request start to signing. |
| `KEEPER_JUPITER_PRIORITY_FEE_LAMPORTS` | `0`; at most `100000` total at the 1.4M CU limit. Replaces API price suggestions. |
| `KEEPER_JUPITER_MAX_SOL_DEBIT_LAMPORTS` | `20000000`; `5000..100000000` maximum simulated payer SOL debit, including rent. |
| `KEEPER_LIQUIDATION_LOOKUP_TABLE` | Optional Dusk table, combined with Jupiter tables. |

Jupiter uses a small geometric partial-size search with a separate full-close
candidate when the repayment cap covers the debt. Full close still needs Dusk's
accepted full preview. This HTTP search is intentionally separate from the
256-probe local Dusk search. Selected instructions are reused, not requoted, for
the final simulation and submission.

Only explicit HTTP 400 `No routes found` is classified as unavailable. Bounds,
packet-size and expected simulation slippage failures can also leave no acceptable
candidate. In lending `both` mode, these permit emergency evaluation. Invalid
responses, unsupported instructions, 429s, outages and pass-budget exhaustion
abort the attempt as retryable errors. 429/5xx responses impose a 30-second
process backoff. A submission failure never selects another mechanism.

## Supported routes and freshness

The first integration accepts Jupiter `route_v2` exact-in instructions with zero
platform/positive-slippage fees, at most eight route steps, and no additional
signer. Shared-account, token-ledger, exact-out and unknown variants fail closed.
Setup permits at most four idempotent canonical ATAs owned by the keeper. Other
instructions, tips and cleanup are rejected. Routes requesting a writable wallet
inside the swap are rejected; some AMMs that create accounts inside their swap
(for example some Manifest routes) therefore need a later reviewed extension.
The keeper never automatically switches to an internal swap on these errors.

Tables are fetched from direct RPC. API-supplied address arrays are ignored.
Tables must have the correct owner/layout, be active, and have completed warmup.
Multiple tables share a single compiled account-index space, with duplicate
addresses loaded once. Signers and invoked programs stay static. This integration
requests transaction version 0 and enforces its 1232-byte packet limit; it does
not implement Solana transaction version 1. Oversized routes are requoted within
the same HTTP budget, never split across transactions.

Protocol state plus route accounts and tables must fit one coherent 100-account
RPC batch. They are read before composite simulation and again before submission;
any fingerprint/fee-epoch change requires a new pass. The full composite is
simulated again before journal submission. Expiry is checked after reservation,
immediately before signing. Expired unsigned work is recorded as abandoned.
Submitted unknown signatures retain the existing exact-byte reconciliation rules,
even when the original route quote has aged out.

## Network and validation limits

The hosted API serves mainnet. The mainnet genesis hash is required. For a local
mainnet fork only, `KEEPER_JUPITER_LOCAL_FORK=1` permits a **shadow** worker against
a loopback HTTP RPC. It does not enable signing or bypass any protocol lock.
The repository's captured devnet deployment must explicitly use
`KEEPER_FLASH_SETTLEMENT=dusk`; mainnet Jupiter pools do not exist on devnet.

Tests cover captured API bytes, on-chain IDL layout, tampered bounds/accounts,
HTTP error classification and budgets, atomic composition across borrowing and
leverage fixtures, both debt sides and partial/full quotes, fee-adjusted payments,
table validation, multi-table execution against pinned Dusk in LiteSVM, and
durable expiry/recovery. The API fixture was read-only: no transaction was signed
or broadcast. These tests do **not** establish an end-to-end Dusk + live Jupiter
pool execution. Before live release, rehearse the full composite on a local
mainnet fork with a configured Dusk market and the selected route pools, including
WSOL, Token-2022, slippage and race scenarios. The frozen protocol and deployed
binary gates remain required.

## Jupiter ABI provenance

`rust/apps/keeper/src/routing/jupiter/aggregator-idl.json` is the complete IDL
published by program `JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4`, read from
Anchor IDL account `C88XWfp26heEmDkmfSzeXP7Fd7GQJ2j9dDTUsyiZbUTa` at mainnet RPC
context slot `466935935` on 2026-11-08. The decompressed original JSON SHA-256 is
`e6c2b425ec740922ee91aa7a3278cea07f4bc00a3ed9efa4eca16facb481bfca` (the checked-in
copy is formatted). The decoder checks the fixed financial prefix; Jupiter
validates the trailing route DAG during the mandatory whole-transaction
simulation. ABI additions require review rather than automatic acceptance.

`build-fixture.json` records a public 0.01 WSOL-to-USDC build with the public
example taker, 50 bps slippage, explicit output ATA, no wrapping, v0 and Whirlpool
routing. Expiring blockhash metadata and unused API address arrays are omitted.
