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

Ordinary execution requires `KEEPER_FLASH_MAX_REPAY`. Jupiter settlement
(`KEEPER_FLASH_SETTLEMENT=jupiter`, the default) routes collateral through the
Swap API V2 and settles externally in one transaction. It requires
`KEEPER_LIQUIDATION_MIN_PROFIT` in debt-token atoms, as does explicit internal
settlement (`KEEPER_FLASH_SETTLEMENT=dusk`). Jupiter is mainnet-only; set `dusk`
explicitly for devnet. See [Jupiter routing](jupiter-routing.md) for configuration,
supported instructions, verification and rollout limits. The leverage worker
uses the same ordinary routing setting.

For an inventory purchase, set `KEEPER_FLASH_SETTLEMENT=inventory`,
`KEEPER_FLASH_MAX_PAYMENT_DEBIT`, and `KEEPER_FLASH_MIN_COLLATERAL_CREDIT`.
It pays the debt token from that worker's wallet and keeps the quoted collateral,
without a swap, market-price comparison, or profit floor. It supports every
discovered supported market; the obsolete `KEEPER_FLASH_MARKET` setting is ignored.
The transfer is grossed up for effective token transfer fees, and candidates that
exceed actual wallet funds or the configured debit bound are skipped so smaller
amounts can be tested. Simulated debt reduction, wallet debit/credit, session
closure, and all on-chain eligibility rules still must pass.

To buy at the program's quoted terms up to available inventory, explicitly set
`KEEPER_FLASH_MAX_REPAY=18446744073709551615`,
`KEEPER_FLASH_MAX_PAYMENT_DEBIT=18446744073709551615`, and
`KEEPER_FLASH_MIN_COLLATERAL_CREDIT=1`. These remove the operator's amount ceilings
while requiring at least one collateral atom; they do not bypass wallet balances
or protocol rules. `KEEPER_LIQUIDATION_MIN_PROFIT` does not filter ordinary
inventory purchases, but is still required for emergency evaluation in `both` mode.

With `KEEPER_LOG_POSITIONS=1`, logs show every preview's amount and full/partial
flag, exact program rejection codes, quoted repayment/fees/gross wallet payment,
available inventory, and collateral credit. Simulation amounts are labeled
`SIMULATED`; only finalized receipts report actual payments and received assets.
An inventory acquisition is reported as tokens paid and collateral received,
without claiming that the collateral is realized profit.

On markets with active hLP, liquidation settlement also supplies the hLP interest
accounting prefix, both asset mints and both reserve vaults. These are required
even for inventory purchases when repayment includes accrued interest. This
does not activate the separate hLP recovery worker or swap the seized collateral.

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

## Simulation fallback

An expected partial execution rejection (including final plan revalidation)
ends partial size searching for that attempt. The keeper discards the quote,
re-reads the position, market, wallet, oracle and fee accounts, and previews
health again before trying a full ordinary liquidation. The full preview must
independently permit closing the position; a partial rejection is not permission.

If no full ordinary route passes and `KEEPER_LENDING_MODE=both`, the keeper
refreshes again and checks emergency eligibility before searching emergency
sizes. The pinned emergency threshold is equity at or below 70% of maintenance.
The on-chain emergency preview and execution simulation must still approve the
size, health improvement, payment and reward bounds. Logs identify each refresh
and explain when emergency is unavailable. This sequence is bounded to one
partial phase, one full phase and one emergency phase per position attempt.

Unexpected program errors, RPC failures and invariant failures abort the attempt.
Once a selected plan enters submission, no failure triggers another route;
pending signatures must be reconciled before a later attempt.

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
