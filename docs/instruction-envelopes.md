# Keeper instruction envelopes

The instruction contract is generated from the exact IDLs and SHA-256 pins in
`protocol.lock.json`. Repository validation regenerates it in memory and fails if
the checked-in contract or fixtures drift.

| Program instruction | Discriminator | IDL accounts |
| --- | --- | ---: |
| `dusk:trigger_liquidation_auction` | `b5ac53586537f66f` | 3 |
| `dusk:bid_liquidation_auction` | `06df1ee493c51b31` | 19 |
| `dusk:settle_liquidation_auction_floor` | `2e5d574e55318979` | 19 |
| `dusk:liquidate_leverage` | `bc840a53ab4e7429` | 19 |
| `dusk:delegated_close_leverage` | `0eda6299a413308b` | 20 |
| `leverage_delegate:before_take_profit` | `05236f00df83c11f` | 8 |
| `leverage_delegate:before_stop_loss` | `f63e52e8a8c7794e` | 8 |
| `leverage_delegate:after_close_order` | `9ce0eefa5fe5eb3b` | 12 |
| `dusk:settle_protocol_auction` | `cecc208708164850` | 15 |
| `dusk:queue_parameter_proposal` | `c259a444fdfb156c` | 7 |
| `dusk:execute_parameter_proposal` | `2e6a707128ff9a1b` | 4 |

## Validation boundary

Rust and TypeScript independently validate the protocol revision, action/job-kind
mapping, program ID, instruction name, discriminator bytes, 32-byte public keys,
required IDL account order, writable/signer flags, optional-account placement,
and fixed token-program addresses. They then hash the exact instruction bytes and
ordered metas with a language-neutral length-prefixed encoding.

The generator also proves each pinned IDL address equals the protocol-lock
program ID and recomputes every eight-byte Anchor `global:<instruction>`
discriminator instead of trusting the IDL field alone.

Delegated TP/SL additionally verifies that before/after hook bytes embedded in
`DelegatedCloseLeverageArgs` equal the two hook envelopes, that
`before_accounts_len` is exact, and that Dusk remaining accounts are the exact
before-then-after account concatenation. The hook envelopes describe CPI inputs;
they are not treated as top-level transactions.

## Explicit ports

The adapter does not discover accounts, derive PDAs, read RPC, evaluate position
health/proposal eligibility, encode business arguments, sign, or submit. Native
`AccountResolver`, `InstructionDataEncoder`, and `EnvelopeSigner` ports make those
boundaries explicit. Account resolution must follow direct-RPC revalidation, and
the remote signer accepts only `SignableJobEnvelope`. That type is constructible
by the adapter only after both envelope validation and the complete frozen-lock
live-readiness gate pass; ordinary `ValidatedJobEnvelope` remains shadow-only.

Passing envelope validation does not prove that a liquidation is healthy, an
auction is profitable, a proposal is eligible/timelocked, or a transaction is
safe to send. Those remain evaluation/policy/postcondition responsibilities. The
captured protocol lock continues to reject live mode regardless of envelope
validity.

For non-delegated instructions, IDLs do not describe dynamic Token-2022 transfer
hook tails. Those remaining metas are checked as valid public keys but their mint
extension-derived order must be produced by the direct-RPC account resolver and
verified through simulation. Argument suffixes are owned by the native generated
encoder; this slice validates their pinned discriminator, while economic bounds
remain typed policy inputs and on-chain constraints.
