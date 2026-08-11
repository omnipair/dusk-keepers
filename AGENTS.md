# Keeper repository guardrails

- `captured` protocol locks allow fixtures, shadow runtimes, and Surfpool work only. A live runtime must require `frozen`, verify every artifact/fingerprint, and pass the explicit live-release gate. Never weaken this check to unblock deployment.
- JSON Schemas, protocol artifacts, and golden fixtures are the cross-language contract. Rust and TypeScript implementations stay native and must pass the same fixtures; do not share implementation source across languages.
- Indexer data may discover candidates but can never authorize execution. Re-read every safety-critical account through direct RPC immediately before evaluation, simulation, and any re-sign.
- Persist the work item, ordered attempt events, signing generation, and outcome in PostgreSQL. Acquire the distributed lease before signing. Live mode must pause when either store is unavailable; never fall back to process-local locking.
- Run lending trigger, lending bidder, lending settler, leverage/order execution, revenue auctions, lifecycle, and sentinel as separate services. Signing services use distinct wallets and remote-signer policies.
- Never store seed phrases, keypair JSON, private keys, database credentials, RPC credentials, or signer tokens in source, logs, health output, fixtures, or plaintext environment templates.
- Blockhash expiry before submit may create a new signing generation only after revalidation. An unknown submitted transaction must be reconciled to landed or definitively not landed before any re-sign.

