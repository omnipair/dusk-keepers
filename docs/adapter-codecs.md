# Native adapter codecs and deterministic account resolution

This slice replaces placeholder instruction encoding with native Rust and
TypeScript implementations. Both runtimes consume the same generated vectors in
`fixtures/conformance/v1/adapter-codec-cases.json`; neither runtime imports the
other's implementation.

## Borsh instruction arguments

The encoder supports exactly the eleven keeper-critical instructions in
`protocol/keeper-instructions.v1.json`:

| Argument layout | Instructions |
| --- | --- |
| no arguments | liquidation trigger, proposal queue, proposal execute |
| two `u64` values | liquidation auction bid |
| four `u64` values | liquidation auction floor settlement |
| one `u8` | leverage liquidation |
| `u8`, `u64`, two Borsh byte vectors, `u16` | delegated leverage close |
| one `u64` | TP hook, SL hook, after-close hook |
| two one-byte enums and two `u64` values | protocol revenue auction settlement |

Rust uses `borsh` derives over native argument structs. TypeScript writes the
same little-endian primitives and Borsh vector lengths independently. The JSON
boundary represents every `u64` as a canonical decimal string so values above
JavaScript's safe-integer limit remain lossless. Native Rust uses `u64`; native
TypeScript uses `bigint`.

Golden bytes are produced by a generic IDL-driven reference encoder, and every
case carries a recursive argument-layout fingerprint. Field-order, nested-struct,
enum-ordinal, or scalar-type drift therefore makes generator freshness fail.

The encoder prepends only a discriminator that matches both the generated
contract and the recomputed Anchor `global:<instruction>` hash. It does not
choose amounts, assets, auction lanes, order IDs, or profitability thresholds.

## PDA and static accounts

`protocol/keeper-account-resolution.v1.json` is generated from the pinned IDLs.
It deduplicates eight PDA recipes:

| Program | PDA recipes |
| --- | --- |
| Dusk | market, borrow position, futarchy authority, event authority, leverage position, leverage collateral vault |
| Leverage delegate | order, custody authority |

Seed encodings are limited to the IDL types present in this keeper surface:
32-byte public keys, 32-byte arrays, and little-endian `u64` values. Rust uses
Curve25519 point decompression and TypeScript independently implements the
Ed25519 field check; both follow Solana's descending bump search and match all
golden addresses and bumps.

The manifest also contains twenty instruction-specific static mappings from IDL
fixed addresses, the instruction's own program account, and the delegated CPI
program selected by the action contract.

## Direct-RPC boundary

Deterministic derivation does not make dynamic seed values trustworthy. Market
mints and parameter hashes, position IDs, order owners, transfer accounts, and
similar values must come from freshly decoded direct-RPC accounts at or above the
requested context slot.

Both adapters expose two unimplemented typed ports:

- `DirectRpcDynamicAccountResolver` returns the requested base accounts with an
  RPC context slot and state hash.
- `Token2022RemainingAccountResolver` returns ordered transfer-hook extra metas
  per typed transfer leg, including the mint, source, destination, authority,
  amount, and decimals used for resolution.

Indexer results may discover work but cannot implement either port. There is no
fallback resolver, signer, submitter, or live executor in this slice. The
captured protocol lock still rejects creation of a signable envelope.

## Regeneration

After an intentional protocol snapshot update, regenerate and review both
semantic diffs:

```bash
node scripts/generate-instruction-contract.mjs --write
node scripts/generate-adapter-codecs.mjs --write
```

Normal repository validation runs both generators without `--write` and fails on
any stale manifest or fixture.
