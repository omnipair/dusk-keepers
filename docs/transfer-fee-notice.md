# Transfer-fee notice monitoring (#287160)

Dusk #45 permits mutable-fee assets. It mitigates an announced fee increase by
restricting new risk immediately and allowing liquidation in the final notice
epoch, before transfers begin paying the new fee. Fee-authority revocation is
not a missing agreed fix. Confirmed keeper execution before activation remains
an operational dependency; issuer-loss risk is not eliminated.

## What this repository implements

The Rust discovery loop reads every market's two mints and Clock directly from
confirmed RPC, in batches of at most 99 mints plus Clock. It repeats on every
`DISCOVERY_INTERVAL_MS` pass (default 15 seconds). Epoch rollover is checked even
when the mint account has not changed. Restarting requires no prior notification
or in-memory event history. An epoch transition across batches fails that scan
and retries on the next pass. A failed notice scan makes readiness degraded;
the prior observation is not used for prioritization. Each liquidation attempt
must still pass its own fresh mint/Clock reads. An unrelated monitor failure
does not block other service profiles.

The lending-trigger and leverage profiles prioritize markets with final-notice
mints, then continue their ordinary candidate scan. They do not cache a healthy
position across scans. Before re-signing an evaluated attempt they re-read both
mints and Clock, at a slot no older than their initial read. A changed fingerprint
causes a retryable outcome and fresh evaluation on the next pass. The fingerprint
includes epoch, authority and schedule changes, including cancellation. Other
mint changes may conservatively invalidate an attempt too. The program remains
the authority on eligibility and actual transfer credit at transaction execution.

Health endpoints expose `feeNotices`: `slot`, `epoch`, `scannedMints`,
`noticeMints`, `finalNoticeMints`, and `finalNoticeMarkets`, plus `feeNoticeError`.
A `fee_notice_final` log is emitted while a final notice is present. This is an
exposure alert, **not proof that every position is eligible or was closed**.

Native Rust and TypeScript fee readers use the same vectors as Dusk SDK's
`transferFeeNotice` / `transferFeeCredit`. TypeScript remains a non-signing scaffold
in this PR; its helper conformance does not establish a TypeScript live executor.

## Epoch example and operator response

For a fee announced at epoch N and effective at N+2:

| Stage | New-risk valuation | Liquidation valuation | Actual transfer fee |
| --- | --- | --- | --- |
| N | Worse of current/pending | Current | Current |
| N+1 | Worse of current/pending | Worse of current/pending | Current |
| N+2 | Effective new fee | Effective new fee | Effective new fee |

“Worse” means the smaller net receipt for that amount, calculating each schedule's
own ceiling-rounded rate and cap. Do not combine maximum rates and caps from
different schedules. A scheduled decrease must not improve health early.
Epochs are read from Clock, not estimated as a fixed number of hours.

1. Alert on any `feeNoticeError` or stale scan. Restore RPC/worker coverage before
   interpreting a zero-send pass as healthy. Sentinel needs the pinned Market layout.
2. On final notice, inspect affected borrowing and leverage positions using the
   matching on-chain previews. Refresh mint schedules, position/market state,
   routing accounts and Clock immediately before evaluation and simulation.
3. Try ordinary eligible flash fills, and the permitted emergency path when its
   separate health gate allows it. Use effective fees for actual route credit and
   every debt-payment gross-up. Notice status does not waive health/fee/reward caps.
4. Confirm inclusion, then re-read remaining debt. Keep unresolved work visible;
   a healthy preview, an observation instruction, or an RPC signature alone is not
   cleanup. Reconcile an unknown submitted transaction before any new signing attempt.
5. Escalate repeated unprofitable/unavailable routes, simulation failures, funding
   failures or activation with exposed debt. Preserve the loss waterfall after
   activation; do not fake net credit or bypass insurance/write-off limits.

A canceled or rescheduled fee requires the same full recheck and can remove
eligibility. Clear stale route/payment estimates. The authority can also cancel
only after a completed early liquidation; that borrower penalty risk remains.

## Deployment compatibility: not completed by monitoring

**The frozen protocol lock in this branch targets the older deployed auction
interface. It does not contain #45's flash/emergency instruction contracts.**
This change adds monitoring and fresh-fee checks to those existing callers. It
does not silently change the pinned deployment, remove live-release gates, or
claim that legacy auction/leverage instructions execute #45's mitigation.

Before enabling a keeper against the eventual #45 program:

1. Capture the final Dusk/delegate binaries, IDLs, SDK and source fingerprint in a
   candidate lock. Generate instruction/layout/account-resolution contracts from
   those IDLs; do not hand-edit the manifests. Candidate testing is non-signing.
2. Migrate the lending and leverage callers to `observeLiquidation`,
   `previewFlashLiquidation`, paired begin/settle, and emergency preview/execution.
   Use the SDK `DuskLiquidations` builders as the TypeScript integration boundary;
   Rust needs its native equivalent and shared contract fixtures. Old auction
   completion does not count as covering the new paths.
3. Exercise both borrowing and leverage with a current 3% fee and scheduled 100%
   uncapped fee: ordinary notice, final notice, activation. Verify exact payment,
   current-fee collateral receipt, confirmed debt reduction, cancellation,
   rescheduling, restart, RPC failure and epoch rollover between quote and signing.
   Include a missed-window case that reaches the authorized loss waterfall.
4. After the coordinated devnet upgrade, attest deployed artifacts, freeze the
   matching lock and pass the live-release gate. Retain database/lease/signing
   policies. Do not claim deployed coverage until these checks actually pass.

These rollout tasks remain explicit. Monitoring is useful now, but it cannot
substitute for the new executor or guarantee economically profitable cleanup.
