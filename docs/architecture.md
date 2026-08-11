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
- **Lending trigger, bidder, and settler:** three independent services and wallets for opening liquidation, bidding into it, and final settlement. Each has its own budget, concurrency, and circuit breaker.
- **Auction arbitrageur:** bids only when the quoted Jupiter exit, fees, priority fee, and conservative slippage leave the configured minimum profit.
- **Lifecycle keeper:** lower-frequency market proposal/activation/LP-lock maintenance jobs; TypeScript may be live after parity and authorization tests.
- **Sentinel:** non-signing liveness, protocol-provenance, slot-lag, circuit-breaker, and Rust/TypeScript parity monitor.

Each live service gets a paired shadow runtime in the other language. Shadow
services write candidates and comparison results but never request signatures.

## Persistence

Postgres is the audit ledger for discoveries, attempts, simulations, signatures,
confirmations, and outcomes. A uniqueness constraint on `(protocol_revision,
candidate_id, expected_state_hash)` makes retries explicit. Redis provides short
leases only; loss of Redis availability pauses new live executions instead of
falling back to in-memory coordination.

## Adapters

Language-native adapters consume the same frozen IDLs, program binaries,
fingerprints, and golden vectors. They do not import a moving checkout of the Dusk
program crate. Adapter responsibilities are account decoding, PDA derivation,
instruction construction, fee-aware arithmetic, and error classification.

The current adapters expose the intended boundary but intentionally do not build
transactions. That fail-closed gap must be completed before live mode can start.
