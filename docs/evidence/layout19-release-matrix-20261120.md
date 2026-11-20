# Layout-19 release evidence, 20 November 2026

This is an evidence index, not a waiver or a frozen-lock decision. The active
revision is `mainnet-2026-11-19`; its lock remains `captured`. No hosted keeper
transaction or mainnet execution canary is claimed here.

## Execution coverage

| Area | Evidence | Remaining limitation |
| --- | --- | --- |
| Liquidation and hLP recovery | `layout19-signed-rehearsal-20261120.json`: 16 signature-verified local committed executions against the pinned executables | These fixture cases do not establish the full Dusk + live Jupiter pool composite. |
| Protection orders | `layout19-signed-orders-20261120.json`: TP and SL at 50% and 100%; repeat cannot pay twice. `protection-skips-runtime.test.ts`: unmet triggers and an already-closed target never submit. | Banks begin after user entry/delegation; the closed-target test is a post-liquidation state, not an actual concurrent liquidation race. |
| Lifecycle | `layout19-signed-lifecycle-20261120.json`: six signed manager/recipient/handoff executions, with and without ALT. `lifecycle-runtime.test.ts`: notice, expiry, successor, epoch and invalid-ALT rejection. | Signed user announcement/acceptance preparation and complete scenario-by-scenario adapter parity are not implied. |
| Continuous auctions | `layout19-signed-auctions-20261120.json`: 48 signed settlements across both token programs and a 1% sold-token transfer fee; actual payment caps, insufficient inventory, competing settlement and recipients checked | This is offline economic/execution evidence, not approval to purchase mainnet revenue or proof of external arbitrage profit. |

## Failure and recovery coverage

`runtime::flash_journal::tests` runs against an isolated real PostgreSQL database.
It verifies lease contention, persisted bytes before submission, cross-revision
pending conflicts, unavailable finalized receipts/history, signer denial, expiry
before and during signing, and recovery without a replacement signature.

The added outage regression terminates only its own PostgreSQL backend before
lease acquisition and during signing. The first case never calls the signer;
the second discards the returned signature without any RPC connection. A new
journal connection reconciles its unsigned reservation without signing.
The uncertain-RPC case retains the same durable signature and transaction bytes
across restart, rebroadcasts only those exact bytes, then reconciles finality
without a new signature. These are synthetic RPC/signer fault tests, not chain
execution receipts.

`runtime::risk::tests::fee_budgets_survive_concurrency_and_restart_while_inventory_is_uncapped`
checks concurrent fee reservations, restart and UTC-day handling. The policy has
SOL fee/reserve limits; it does not impose token inventory budgets.
`liquidation::snapshot_tests` rejects a lagging or inconsistent RPC bank, while
`oracle-accounts.test.ts` exercises missing, stale and wrong-owner reports against
the actual pinned program. `mainnet-gate.test.ts` and native deployment tests
reject wrong genesis, captured live execution and changed program authorities,
slots, code or ProgramData references.

The native hosted runtime discovers work on an HTTP RPC polling loop
(`rust/apps/keeper/src/main.rs`, `runtime/rpc.rs`). It does not depend on Redis
or a WebSocket subscription. A Redis or WebSocket fault therefore cannot be
claimed as an exercised native dependency. The runbook's broader fault/parity
matrix still needs an explicit final applicability review; these observations
do not silently mark it complete.

## Hosted observations and activation blockers

Earlier observations on 20 November verified five distinct worker task roles
and KMS Ed25519 non-transaction challenges. Each signing wallet was funded with
0.1 SOL in finalized transaction
`3rRPZoZmnFbCTxK4eDt7BhUFK3qAoqk6RVLHUBpoBN2gLVFev3okKGacpmihSEyjRdKXjqmSMEsyV48yQQTjfKLd`.
All six services were healthy in shadow mode with zero submitted transactions.
These are dated observations, not a fresh status check. Their deployment receipts
are retained privately with the release proof.

Before activation:

1. Deploy and verify the corrected auction token-program image in shadow mode.
2. Complete the remaining actual liquidation/order race, selected Jupiter-route
   rehearsal and complete scenario-level Rust/TypeScript parity review.
3. Review the complete evidence before freezing the exact lock and passing the
   explicit live-release workflow; the old revision's waiver is inapplicable.
4. Obtain token-capital approval and set each spending profile's execution bounds.
5. Run one controlled, owner-authorized mainnet keeper execution and reconcile its
   finalized transaction, state and durable fee reservation before broader activation.
