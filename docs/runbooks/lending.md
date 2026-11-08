# Lending keeper

Run one service with `KEEPER_PROFILE=lending`. It discovers borrowing positions,
reads both debt sides directly from RPC, and previews current health. The removed
`lending-trigger`, `lending-bidder`, and `lending-settler` profiles fail startup;
update existing service configuration before deploying this revision.

## Modes

| `KEEPER_LENDING_MODE` | Behavior |
| --- | --- |
| `both` (default) | Try ordinary partial/full flash liquidation, then permitted emergency partial/full settlement if no acceptable ordinary route exists. |
| `ordinary` | Only ordinary flash liquidation. |
| `emergency` | Only emergency settlement; ordinary repayment and inventory bounds are not required. |

Keep `KEEPER_MODE=shadow` with a `KEEPER_SHADOW_PUBLIC_KEY` to discover and simulate
without signing. The captured protocol lock still blocks live startup. This
devnet artifact update does not grant live release permissions. See the
[devnet capture record](../devnet-capture.md) for the pinned deployment.

## Economic bounds

Ordinary execution requires `KEEPER_FLASH_MAX_REPAY`. Internal Dusk settlement
(`KEEPER_FLASH_SETTLEMENT=dusk`, the default) also requires
`KEEPER_LIQUIDATION_MIN_PROFIT` in debt-token atoms. Inventory settlement requires
`KEEPER_FLASH_MARKET`, `KEEPER_FLASH_MAX_PAYMENT_DEBIT`, and
`KEEPER_FLASH_MIN_COLLATERAL_CREDIT`. The market restriction scopes inventory
purchases only; in `both` mode, other markets can still receive emergency evaluation.

Emergency execution independently requires `KEEPER_LIQUIDATION_MIN_PROFIT`.
`KEEPER_EMERGENCY_SLIPPAGE_BPS` optionally tolerates movement between the preview
and execution quotes for both the reward and swap output. It defaults to `0`
(exact quotes), accepts `0..10000` basis points, and never lowers the transaction's
reward floor below `KEEPER_LIQUIDATION_MIN_PROFIT`. For example, `10` allows 0.1%.
`10000` allows 100%: the quote-derived reward and swap-output floors become zero,
while the configured minimum profit remains enforced on chain.
An inventory purchase bound does not replace the emergency reward/profit bound.
Full liquidation always requires an accepted full preview, regardless of whether
partial searches found a route. Both mechanisms retain their pinned protocol
eligibility and loss-allocation rules.

## Submission and recovery

Route selection finishes before submission. An unexpected preview, RPC, or
invariant error aborts evaluation. A selected plan is submitted once through the
PostgreSQL journal; a submission timeout or unknown signature never triggers the
other mechanism. Recovery reconciles pending signatures before new work. All
instances must share the same `DATABASE_URL` to coordinate position/debt-side
locks and pending transactions.

Configure the wallet, SOL reserve, per-pass budget and optional address lookup
table as for the prior executors. When switching from two instances to one,
remove the old deployments and preserve their journal database until pending
transactions have been reconciled. Do not discard pending records to start the
new service.
