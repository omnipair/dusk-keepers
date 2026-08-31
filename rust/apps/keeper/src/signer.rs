//! Signing.
//!
//! A trait rather than a concrete keypair, because the devnet arrangement — a
//! file on disk — is the one arrangement that must not survive to mainnet. A
//! remote signer, an HSM, or a policy-gated service implements the same two
//! methods, so swapping one in later changes deployment configuration rather
//! than execution code.
//!
//! The trait is deliberately narrow: it signs opaque bytes and names its
//! public key. It cannot be asked what it is signing, which keeps the decision
//! of *whether* to sign in the execution path where the simulation result and
//! the policy bounds live.

use std::{error::Error, fmt, fs, path::Path};

use ed25519_dalek::{Signer as _, SigningKey};

#[derive(Debug)]
pub enum SignerError {
    Unreadable(String),
    Malformed(String),
}

impl fmt::Display for SignerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(detail) => write!(formatter, "signer key unreadable: {detail}"),
            Self::Malformed(detail) => write!(formatter, "signer key malformed: {detail}"),
        }
    }
}

impl Error for SignerError {}

pub trait TransactionSigner: Send + Sync {
    /// The signing public key, base58 encoded, as it appears in an account list.
    fn public_key(&self) -> [u8; 32];

    fn sign(&self, message: &[u8]) -> [u8; 64];

    fn public_key_base58(&self) -> String {
        bs58::encode(self.public_key()).into_string()
    }
}

/// A keypair held in this process, loaded from a Solana CLI keypair file.
///
/// Correct for devnet and nowhere else: the key is readable by anything that
/// can read the container's filesystem.
pub struct LocalKeypair {
    key: SigningKey,
}

impl LocalKeypair {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, SignerError> {
        let raw = fs::read_to_string(path.as_ref())
            .map_err(|error| SignerError::Unreadable(error.to_string()))?;
        Self::from_json(&raw)
    }

    /// Accepts the Solana CLI's 64-byte array: 32 bytes of seed followed by
    /// the 32-byte public key. The trailing half is checked rather than
    /// trusted, so a truncated or spliced file fails here instead of
    /// producing signatures no one can verify.
    pub fn from_json(raw: &str) -> Result<Self, SignerError> {
        let bytes: Vec<u8> = serde_json::from_str(raw)
            .map_err(|error| SignerError::Malformed(error.to_string()))?;
        if bytes.len() != 64 {
            return Err(SignerError::Malformed(format!(
                "expected 64 bytes, found {}",
                bytes.len()
            )));
        }
        let mut seed = [0_u8; 32];
        seed.copy_from_slice(&bytes[..32]);
        let key = SigningKey::from_bytes(&seed);
        if key.verifying_key().to_bytes() != bytes[32..] {
            return Err(SignerError::Malformed(
                "public key half does not match the secret half".into(),
            ));
        }
        Ok(Self { key })
    }

    /// Read the key from an environment variable holding the same JSON array.
    /// Railway has no filesystem to mount a key onto, so this is how a
    /// deployed keeper receives one.
    pub fn from_environment(variable: &str) -> Result<Self, SignerError> {
        let raw = std::env::var(variable)
            .map_err(|_| SignerError::Unreadable(format!("{variable} is not set")))?;
        Self::from_json(&raw)
    }
}

impl TransactionSigner for LocalKeypair {
    fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        let key = SigningKey::from_bytes(&[7_u8; 32]);
        let mut bytes = key.to_bytes().to_vec();
        bytes.extend_from_slice(&key.verifying_key().to_bytes());
        serde_json::to_string(&bytes).unwrap()
    }

    #[test]
    fn loads_a_solana_keypair_file() {
        let keypair = LocalKeypair::from_json(&sample()).unwrap();
        assert_eq!(keypair.public_key().len(), 32);
        assert_eq!(keypair.sign(b"payload").len(), 64);
    }

    #[test]
    fn rejects_a_mismatched_public_half() {
        let mut bytes: Vec<u8> = serde_json::from_str(&sample()).unwrap();
        bytes[40] ^= 0xff;
        let raw = serde_json::to_string(&bytes).unwrap();
        assert!(matches!(
            LocalKeypair::from_json(&raw),
            Err(SignerError::Malformed(_))
        ));
    }

    #[test]
    fn rejects_a_truncated_key() {
        assert!(LocalKeypair::from_json("[1,2,3]").is_err());
    }
}
