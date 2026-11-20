# Hosted KMS readiness

Before activating a signing profile, run `/usr/local/bin/dusk-keeper --check-kms-signer` inside that profile's existing AWS task, retaining its own task-role credentials, KMS key binding and `KEEPER_MODE=shadow`. Do not run the command using an administrator's credentials and call that proof of worker access.

The command signs only a generated JSON challenge bound to `DUSK_KEEPER_KMS_READINESS_V1`, the profile, configured public key and a fresh timestamp nonce. It verifies the Ed25519 signature locally and prints the public challenge/signature proof. It accepts no message, transaction or extra CLI argument; it never starts discovery, acquires a signing generation or sends an RPC transaction. A denied signing call, mismatched key or invalid signature fails the check. Sentinel and local-key profiles cannot use this command.

Keep each public proof with the release evidence, including task ARN, image digest, source commit and observation time. Re-check the five distinct worker identities. A successful canary does not freeze the captured layout-19 lock or satisfy the signed Surfpool release matrix, journal/lease recovery, token inventory or on-chain canary gates.

The native image packages `deploy/mainnet/risk-policy.json` at `/app/config/risk-policy.json` and selects that path through `KEEPER_RISK_POLICY`. The owner approved 0.0002 SOL per transaction, 0.02 SOL per UTC day and a 0.05 SOL reserve per signing wallet on 2026-11-20. These caps cover fees only. They do not fund or authorize token purchases or automatically enable any worker. Shadow remains the image default; existing live-mode validation is unchanged.
