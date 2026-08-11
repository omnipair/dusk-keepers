# Keeper threat model

## Protected assets

- Service-wallet funds and signing authority.
- User positions that can be closed or liquidated.
- Protocol auction inventory and revenue.
- Price, account, and protocol-revision integrity used to decide execution.
- Attempt history needed for incident reconstruction.

## Trust boundaries

RPC providers, the indexer, Jupiter quotes, Railway, Postgres, Redis, and the remote
signer are separate trust domains. No single off-chain source is authoritative for
execution eligibility. Dusk program state re-read from RPC and the frozen protocol
adapter are the final inputs; simulation remains advisory and must not replace
on-chain constraints.

## Primary threats and controls

| Threat | Required control |
| --- | --- |
| Stale or malicious indexer data | Direct RPC revalidation; maximum slot age; state hash in every candidate |
| Duplicate or competing workers | Redis lease plus durable uniqueness key; expected races become typed skips |
| Compromised signer credential | Remote signer; service-specific key; allowlist, spend/rate limits, immediate revocation |
| ABI/account-layout drift | Frozen artifact hashes and semantic fingerprints; adapters reject revision mismatch |
| Oracle/RPC/Jupiter manipulation | Multiple-source freshness checks; conservative bounds; simulation; on-chain limit checks |
| Transaction replay or blockhash expiry | Persist attempt/blockhash; explicit expiry classification; rebuild only after revalidation |
| Poisoned dependencies or build image | Locked dependencies; reviewed container digests before release; SBOM and vulnerability scan gate |
| Secret leakage through logs/health | Structured allowlisted fields; no transaction bytes, signer material, URLs, or headers |

## Job-specific controls

Liquidations require fresh health inputs, maximum repay sizing, minimum received
collateral, and a final account-version check. TP/SL requires the delegated order
to still exist, the trigger to remain met, and the position/order relationship to
match. Auction bids require minimum net profit after all fees and a capped maximum
inventory exposure. Lifecycle work requires explicit on-chain authorization and
cannot share a wallet with liquidators or the revenue bidder.

## Fail-closed conditions

Pause new signatures when the protocol revision differs, the RPC slot lags, the
oracle is stale, Redis or Postgres is unavailable, balance falls below reserve,
failure rate breaches the circuit breaker, or Rust/TypeScript shadow parity drifts.
Already-submitted signatures continue through confirmation reconciliation.

## Remaining work before live mode

The scaffold has no signer, RPC sender, PostgreSQL/Redis runtime implementation,
oracle policy, Jupiter route verifier, or transaction adapter. These are explicit
launch blockers, not configuration toggles.
