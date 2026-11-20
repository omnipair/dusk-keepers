//! A domain-bound remote signer diagnostic. No RPC, journal or transaction sender exists here.
use super::{SignerError, TransactionSigner};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Serialize;

const DOMAIN: &str = "DUSK_KEEPER_KMS_READINESS_V1";
const PROFILES: [&str; 5] = [
    "lending",
    "leverage",
    "hlp-recovery",
    "auction-arbitrage",
    "lifecycle",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Challenge<'a> {
    domain: &'static str,
    profile: &'a str,
    public_key: String,
    nonce: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proof {
    schema_version: u8,
    kind: &'static str,
    profile: String,
    public_key: String,
    message: String,
    signature: String,
    signature_verified: bool,
    transaction_submitted: bool,
}

pub fn check(
    signer: &dyn TransactionSigner,
    profile: &str,
    nonce: &str,
) -> Result<Proof, SignerError> {
    if !PROFILES.contains(&profile)
        || nonce.is_empty()
        || nonce.len() > 39
        || !nonce.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(SignerError::Remote("invalid KMS readiness challenge"));
    }
    let public_key = signer.public_key_base58();
    let message = serde_json::to_string(&Challenge {
        domain: DOMAIN,
        profile,
        public_key: public_key.clone(),
        nonce,
    })
    .map_err(|_| SignerError::Remote("unable to encode KMS readiness challenge"))?;
    let signed = signer.sign(message.as_bytes())?;
    let key = VerifyingKey::from_bytes(&signer.public_key())
        .map_err(|_| SignerError::Remote("invalid KMS readiness public key"))?;
    key.verify_strict(message.as_bytes(), &Signature::from_bytes(&signed))
        .map_err(|_| SignerError::Remote("KMS readiness signature did not verify"))?;
    Ok(Proof {
        schema_version: 1,
        kind: "non-transaction-kms-readiness",
        profile: profile.into(),
        public_key,
        message,
        signature: bs58::encode(signed).into_string(),
        signature_verified: true,
        transaction_submitted: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    struct TestSigner {
        bad: bool,
        denied: bool,
    }
    impl TransactionSigner for TestSigner {
        fn public_key(&self) -> [u8; 32] {
            SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes()
        }
        fn sign(&self, message: &[u8]) -> Result<[u8; 64], SignerError> {
            if self.denied {
                return Err(SignerError::Remote("signing denied"));
            }
            let mut signature = SigningKey::from_bytes(&[7; 32]).sign(message).to_bytes();
            if self.bad {
                signature[0] ^= 1;
            }
            Ok(signature)
        }
    }
    #[test]
    fn proof_is_bound_to_profile_key_and_fresh_challenge_and_contains_no_transaction() {
        let signer = TestSigner {
            bad: false,
            denied: false,
        };
        let proof = check(&signer, "leverage", "123").unwrap();
        let message: serde_json::Value = serde_json::from_str(&proof.message).unwrap();
        assert_eq!(message["domain"], DOMAIN);
        assert_eq!(message["profile"], "leverage");
        assert_eq!(message["publicKey"], signer.public_key_base58());
        assert_eq!(message["nonce"], "123");
        assert_ne!(
            proof.signature,
            check(&signer, "leverage", "124").unwrap().signature
        );
        assert_ne!(
            proof.signature,
            check(&signer, "lifecycle", "123").unwrap().signature
        );
        assert!(proof.signature_verified);
        assert!(!proof.transaction_submitted);
    }
    #[test]
    fn refuses_forged_remote_signatures_and_signer_denial() {
        assert!(
            check(
                &TestSigner {
                    bad: true,
                    denied: false
                },
                "lending",
                "1"
            )
            .is_err()
        );
        assert!(
            check(
                &TestSigner {
                    bad: false,
                    denied: true
                },
                "lending",
                "1"
            )
            .is_err()
        );
    }
    #[test]
    fn refuses_arbitrary_payloads_and_non_signing_profiles_before_signing() {
        let signer = TestSigner {
            bad: false,
            denied: true,
        };
        for profile in ["sentinel", "conformance", "leverage\ntransaction"] {
            assert_eq!(
                check(&signer, profile, "1").err().unwrap().to_string(),
                "remote signer: invalid KMS readiness challenge"
            );
        }
        for nonce in ["", "transaction-bytes", "1\n2"] {
            assert_eq!(
                check(&signer, "leverage", nonce).err().unwrap().to_string(),
                "remote signer: invalid KMS readiness challenge"
            );
        }
    }
}
