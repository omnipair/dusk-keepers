# Architecture

## Invariants

1. A candidate is valid for exactly one `protocolRevision` and one observed state hash.
2. Discovery may use the indexer, but execution re-reads all safety-critical accounts from RPC.
3. A distributed lease and durable attempt row exist before signing; the in-process scheduler is not the lock of record.
4. Every transaction is simulated, bounded, submitted, confirmed, and followed by a state postcondition check.
5. Rust and TypeScript produce the same candidate IDs, priorities, instruction intents, and outcome reason codes from shared fixtures.

## Runtime flow

```mermaid
flowchart LR
    I["Indexer / RPC discovery"] --> C["Candidate intent"]
    C --> A["Durable attempt row"]
    A --> L["Distributed lease"]
    L --> R["Direct RPC revalidation"]
    R --> B["Pinned adapter builds intent"]
    B --> S["Simulate and enforce bounds"]
    S --> K["Remote signer policy"]
    K --> T["Submit and confirm"]
    T --> P["Postcondition re-read"]
    P --> O["Execution outcome"]
```

Candidates are deduplicated by `conflictKey`, not by process-local task ID. A
position, lending obligation, auction, or lifecycle target therefore cannot be
executed concurrently by different job profiles.

## Service boundaries

- **Leverage keeper:** leverage liquidation plus TP/SL. Liquidation supersedes stop-loss, which supersedes take-profit for a position.
- **Lending keeper:** one discovery and health pass selects an ordinary liquidation first, then an emergency route only if no acceptable ordinary route exists. `KEEPER_LENDING_MODE=ordinary|emergency|both` restricts selection. Errors abort selection; submission never falls through to another route. Health monitoring is available in shadow mode.
- **Auction arbitrageur:** bids only when the quoted Jupiter exit, fees, priority fee, and conservative slippage leave the configured minimum profit.
- **Lifecycle keeper:** lower-frequency announced operator-bundle execution and revenue archival; TypeScript may be live after parity and authorization tests.
- **Sentinel:** non-signing liveness, protocol-provenance, slot-lag, circuit-breaker, and Rust/TypeScript parity monitor.

Each live service gets a paired shadow runtime in the other language. Shadow
services write candidates and comparison results but never request signatures.

## Persistence

Postgres is the audit ledger for discoveries, attempts, simulations, signatures,
confirmations, and outcomes. A uniqueness constraint on `(protocol_revision,
candidate_id, expected_state_hash)` makes retries explicit. The liquidation/order submission path uses PostgreSQL advisory locks and a durable
transaction journal. Pending signatures are reconciled before replacement. Redis
is not used by this runtime.

## Adapters

Language-native adapters consume the same frozen IDLs, program binaries,
fingerprints, and golden vectors. They do not import a moving checkout of the Dusk
program crate. Adapter responsibilities are account decoding, PDA derivation,
instruction construction, fee-aware arithmetic, and error classification.

Rust builds and simulates native transactions. TypeScript provides independent
codecs and reference checks; its service remains a non-signing scaffold. The
captured protocol lock blocks live startup in both runtimes.

## Source boundaries

The native entry points are `rust/apps/keeper/src/workers/{lending,leverage,hlp,
revenue,lifecycle,sentinel}/`. Each folder owns its domain's discovery or execution
orchestration; leverage also owns delegated orders and hLP owns its vault plans.
`workers/mod.rs` is the only profile dispatcher.

`liquidation/` contains the shared borrowing/leverage engine, ordinary/emergency
selection policy, atomic instruction plans, bounded size search, simulation and
snapshot validation. Route selection returns one plan before submission starts.
`runtime/` contains RPC transport, account resolution, the signer interface,
transaction wire encoding and PostgreSQL submission/recovery. `config.rs` and
`health.rs` keep configuration and HTTP reporting out of the workers.

The six deployable profiles are lending, leverage, hLP recovery, revenue auctions,
operator lifecycle and sentinel. The TypeScript service remains a scaffold;
its codecs and shared fixtures validate protocol parity, not a second executor.
