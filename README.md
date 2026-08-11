# Dusk Keepers

Production-oriented keeper runtimes for Dusk, pinned to one protocol snapshot and
verified with the same conformance fixtures in Rust and TypeScript.

This repository is a scaffold. It deliberately defaults to **shadow mode**. The
Local Snapshot 0 artifacts are captured and hash-verified, but the lock remains
`captured` until the complete Surfpool matrix passes. Live execution additionally
requires a real executor implementation. No signer material belongs in this
repository or in plaintext environment variables.

## What is here

- `protocol/`: language-neutral JSON Schemas and protocol compatibility rules.
- `protocol/keeper-instructions.v1.json`: mechanically generated critical-instruction contract pinned to both IDLs.
- `fixtures/`: golden inputs shared by the Rust and TypeScript test suites.
- `rust/`: the first live-runtime target; currently a safe, non-signing skeleton.
- `typescript/`: the shadow/reference runtime and future lifecycle workers.
- `deploy/railway/`: separate Railway service profiles and hardened container builds.
- `docs/`: architecture, threat model, upgrade process, and operational runbooks.

The colleague-built `leverage-execution-bot` informed priority, deduplication,
race classification, and postcondition behavior. It is not copied or treated as
the canonical protocol client.

## Start here

```bash
node scripts/validate-repo.mjs
cargo test --locked --workspace
npm test
```

All three commands are designed to run without a local validator. Rust dependency
resolution can run offline after the two small serialization crates are cached:

```bash
cargo test --locked --workspace --offline
```

Instruction contract drift is fail-fast. Regenerate only after intentionally
changing the pinned protocol snapshot:

```bash
node scripts/generate-instruction-contract.mjs --write
```

## Safety state

`protocol.lock.json` is `captured`: all program, IDL, SDK, and compatibility
hashes are real and validated, while both adapters still reject live mode. Move
it to `frozen` only through
[`docs/runbooks/protocol-upgrade.md`](docs/runbooks/protocol-upgrade.md) after the
full Surfpool and cross-runtime gates pass.
