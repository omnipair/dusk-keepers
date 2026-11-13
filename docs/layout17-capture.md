# Layout 17 captured contract

Source [Dusk PR 106](https://github.com/omnipair/dusk/pull/106), commit
`c24e143f0fd16fa72772fe0c78e149309cf47738`, SDK `2.23.1-next.20261114`.
The standard capture generator pins fresh binaries, both generated IDLs, SDK
archive and compatibility fingerprints. Both binaries embed the source commit.
Instruction specifications, codecs, offsets and liquidation/hLP bank fixtures
were regenerated from that source. This is a source capture, not a deployment.

The Market account includes two signed i128 funding translations. Their
presence changes subsequent offsets and total account size. Rust and
TypeScript regressions read the same signed fixture and verify the trailing
operator/slot and ownership fields remain aligned. Pricing translations are
not balances and must not enter a keeper's collateral or NAV estimate.

The delegate uses collateral/debt EMA thresholds and an explicit positive
minimum net owner payout. Its execution fixtures use those fields. Price
trigger checks and minimum proceeds remain the program's decision; discovery
data never authorizes execution.

The captured lock still permits offline/shadow work only. The previous devnet
deployment observation was removed because these binaries have not been
observed on-chain. Runtime devnet tests create isolated mock evidence, never a
release attestation. Recapture actual deployment evidence before any explicitly
authorized devnet execution; normal live use still requires the frozen lock,
artifact verification, journaling and release gate.
