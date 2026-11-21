# Remote readiness telemetry

The Rust keeper emits one JSON line every30seconds with `event: "dusk_keeper_health"` and `schemaVersion: 1`. It shares the exact `/readyz` decision. An independent reporting thread detects an old completed scan even if the discovery/execution loop stalls. Existing health endpoints and execution policies retain their behavior.

Fields: `profile`, `protocolRevision`, `observedAtMs` (UTC Unix milliseconds), `status` (`ready`, `starting`, `degraded`), generic `reason`, `scanCount` and `sentCount` as decimal u64 strings, `lastCompletedScanAgeMs`, and `confirmedCount: null`.

`sentCount` is a process counter and includes setup transactions. It cannot prove a confirmed protocol action. Confirmed actions remain unavailable until execution receipts distinguish protocol actions from setup. Counters restart with the process. No RPC reads, submissions, public ports, private keys, signing capability, URLs, raw errors or credentials are added by reporting. CloudWatch's existing stdout log driver collects the report.

Collectors must match schema, profile and the currently running task's log stream, reject future timestamps and mark a heartbeat older than120seconds stale. RUNNING and live mode are not readiness. No report means unavailable. Deployment of this source is separate from source merge; no live mode, inventory, fee or reserve setting changes are part of this PR.

The dashboard integration lives in the internal-dashboard v2 PR. The extra small log volume stays in the existing log-retention policy.

Tests cover parity with HTTP readiness, starting/stale/error/configuration rejection, sentinel's no-signer behavior, exclusion of raw credentials and unknown confirmation counts. Two pre-existing local RPC mock servers explicitly clear the accepted socket's inherited nonblocking flag for macOS portability; their assertions and production RPC code are unchanged.
