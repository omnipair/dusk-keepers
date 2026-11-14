//! Non-exportable AWS KMS Ed25519 signing. AWS credentials use the standard
//! provider chain; neither private key material nor service errors are logged.
use super::{SignerError, TransactionSigner};
use aws_sdk_kms::{
    Client,
    config::{BehaviorVersion, Region},
    primitives::Blob,
    types::{KeySpec, KeyUsageType, MessageType, SigningAlgorithmSpec},
};
use ed25519_dalek::{Signature, VerifyingKey};
use std::{env, time::Duration};
use tokio::runtime::Runtime;

const DEADLINE: Duration = Duration::from_secs(15);
const ED25519_SPKI: &[u8] = &[
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

pub struct AwsKmsSigner {
    client: Client,
    runtime: Runtime,
    key_arn: String,
    public_key: [u8; 32],
}

fn required(name: &str) -> Result<String, SignerError> {
    env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| SignerError::Unreadable(format!("{name} is required")))
}

fn key_region(arn: &str) -> Result<&str, SignerError> {
    let parts: Vec<_> = arn.split(':').collect();
    if parts.len() != 6
        || parts[0] != "arn"
        || parts[1] != "aws"
        || parts[2] != "kms"
        || parts[3].is_empty()
        || parts[4].len() != 12
        || !parts[4].bytes().all(|b| b.is_ascii_digit())
        || !parts[5].starts_with("key/")
        || parts[5].len() != 40
        || !parts[5][4..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return Err(SignerError::Remote(
            "SIGNER_KEY_ID must be an immutable KMS key ARN",
        ));
    }
    Ok(parts[3])
}

fn public_key(der: &[u8], expected: &[u8; 32]) -> Result<[u8; 32], SignerError> {
    if der.len() != 44 || !der.starts_with(ED25519_SPKI) {
        return Err(SignerError::Remote("KMS public key is not Ed25519 SPKI"));
    }
    let key: [u8; 32] = der[12..]
        .try_into()
        .map_err(|_| SignerError::Remote("invalid KMS public key"))?;
    if &key != expected || VerifyingKey::from_bytes(&key).is_err() {
        return Err(SignerError::Remote(
            "KMS public key differs from KEEPER_SIGNER_PUBLIC_KEY",
        ));
    }
    Ok(key)
}

fn checked_signature(
    key: &[u8; 32],
    message: &[u8],
    bytes: &[u8],
) -> Result<[u8; 64], SignerError> {
    let signature = Signature::from_slice(bytes)
        .map_err(|_| SignerError::Remote("invalid KMS signature length"))?;
    VerifyingKey::from_bytes(key)
        .map_err(|_| SignerError::Remote("invalid signer public key"))?
        .verify_strict(message, &signature)
        .map_err(|_| SignerError::Remote("KMS signature verification failed"))?;
    Ok(signature.to_bytes())
}

impl AwsKmsSigner {
    pub fn from_environment() -> Result<Self, SignerError> {
        let key_arn = required("SIGNER_KEY_ID")?;
        let region = required("AWS_REGION")?;
        if key_region(&key_arn)? != region {
            return Err(SignerError::Remote(
                "AWS_REGION differs from the KMS key ARN",
            ));
        }
        let expected: [u8; 32] = bs58::decode(required("KEEPER_SIGNER_PUBLIC_KEY")?)
            .into_vec()
            .map_err(|_| SignerError::Remote("invalid KEEPER_SIGNER_PUBLIC_KEY"))?
            .try_into()
            .map_err(|_| SignerError::Remote("invalid KEEPER_SIGNER_PUBLIC_KEY"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SignerError::Remote("unable to initialize signing runtime"))?;
        let config = runtime
            .block_on(async {
                tokio::time::timeout(
                    DEADLINE,
                    aws_config::defaults(BehaviorVersion::latest())
                        .region(Region::new(region))
                        .retry_config(
                            aws_config::retry::RetryConfig::standard().with_max_attempts(1),
                        )
                        .load(),
                )
                .await
            })
            .map_err(|_| SignerError::Remote("AWS credential/configuration deadline exceeded"))?;
        let client = Client::new(&config);
        Self::from_client(runtime, client, key_arn, expected)
    }

    fn from_client(
        runtime: Runtime,
        client: Client,
        key_arn: String,
        expected: [u8; 32],
    ) -> Result<Self, SignerError> {
        let response = runtime
            .block_on(async {
                tokio::time::timeout(DEADLINE, client.get_public_key().key_id(&key_arn).send())
                    .await
            })
            .map_err(|_| SignerError::Remote("KMS public-key deadline exceeded"))?
            .map_err(|_| SignerError::Remote("KMS GetPublicKey denied or unavailable"))?;
        if response.key_id() != Some(key_arn.as_str())
            || response.key_spec() != Some(&KeySpec::EccNistEdwards25519)
            || response.key_usage() != Some(&KeyUsageType::SignVerify)
            || !response
                .signing_algorithms()
                .contains(&SigningAlgorithmSpec::Ed25519Sha512)
        {
            return Err(SignerError::Remote(
                "KMS key identity, usage or signing algorithm mismatch",
            ));
        }
        let public_key = public_key(
            response
                .public_key()
                .ok_or(SignerError::Remote("missing KMS public key"))?
                .as_ref(),
            &expected,
        )?;
        Ok(Self {
            client,
            runtime,
            key_arn,
            public_key,
        })
    }
}

impl TransactionSigner for AwsKmsSigner {
    fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    fn sign(&self, message: &[u8]) -> Result<[u8; 64], SignerError> {
        if message.is_empty() || message.len() > 1232 {
            return Err(SignerError::Remote("invalid Solana message length"));
        }
        // Solana needs pure Ed25519 over the original message, not Ed25519ph.
        let response = self
            .runtime
            .block_on(async {
                tokio::time::timeout(
                    DEADLINE,
                    self.client
                        .sign()
                        .key_id(&self.key_arn)
                        .message(Blob::new(message))
                        .message_type(MessageType::Raw)
                        .signing_algorithm(SigningAlgorithmSpec::Ed25519Sha512)
                        .send(),
                )
                .await
            })
            .map_err(|_| SignerError::Remote("KMS signing deadline exceeded"))?
            .map_err(|_| SignerError::Remote("KMS Sign denied or unavailable"))?;
        if response.key_id() != Some(self.key_arn.as_str())
            || response.signing_algorithm() != Some(&SigningAlgorithmSpec::Ed25519Sha512)
        {
            return Err(SignerError::Remote("KMS signing key or algorithm changed"));
        }
        checked_signature(
            &self.public_key,
            message,
            response
                .signature()
                .ok_or(SignerError::Remote("missing KMS signature"))?
                .as_ref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::transaction::base64;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn only_the_pinned_ed25519_spki_is_accepted() {
        let key = SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes();
        let mut der = [ED25519_SPKI, &key].concat();
        assert_eq!(public_key(&der, &key).unwrap(), key);
        assert!(public_key(&der, &[0; 32]).is_err());
        der[8] ^= 1;
        assert!(public_key(&der, &key).is_err());
        assert!(public_key(&der[..43], &key).is_err());
    }

    #[test]
    fn remote_signatures_are_verified_against_the_exact_message() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let message = b"transaction message";
        let signature = key.sign(message).to_bytes();
        assert_eq!(
            checked_signature(&key.verifying_key().to_bytes(), message, &signature).unwrap(),
            signature
        );
        assert!(
            checked_signature(
                &key.verifying_key().to_bytes(),
                b"different message",
                &signature
            )
            .is_err()
        );
        assert!(
            checked_signature(&key.verifying_key().to_bytes(), message, &signature[..63]).is_err()
        );
        assert!(checked_signature(&[0; 32], message, &signature).is_err());
    }

    #[test]
    fn key_aliases_and_cross_region_arns_cannot_be_immutable_identities() {
        assert_eq!(
            key_region(
                "arn:aws:kms:us-west-2:123456789012:key/12345678-1234-1234-1234-123456789012"
            )
            .unwrap(),
            "us-west-2"
        );
        for value in [
            "alias/keeper",
            "12345678-1234-1234-1234-123456789012",
            "arn:aws:kms:us-west-2:123456789012:alias/keeper",
        ] {
            assert!(key_region(value).is_err());
        }
    }

    #[test]
    fn sdk_sends_raw_ed25519_and_rejects_denial_or_a_wrong_message_signature() {
        for mode in ["success", "denied", "wrong_message"] {
            let key_arn =
                "arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012";
            let key = SigningKey::from_bytes(&[7; 32]);
            let expected = key.verifying_key().to_bytes();
            let message = b"exact Solana message bytes";
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let server = std::thread::spawn(move || {
                for step in 0..2 {
                    let deadline = std::time::Instant::now() + Duration::from_secs(10);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(
                                    std::time::Instant::now() < deadline,
                                    "missing SDK request"
                                );
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("mock KMS accept: {error}"),
                        }
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut raw = Vec::new();
                    let mut buffer = [0u8; 4096];
                    let body = loop {
                        let read = stream.read(&mut buffer).unwrap();
                        assert!(read > 0);
                        raw.extend_from_slice(&buffer[..read]);
                        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                            let length: usize = headers
                                .lines()
                                .find_map(|line| line.strip_prefix("content-length:"))
                                .unwrap()
                                .trim()
                                .parse()
                                .unwrap();
                            if raw.len() >= end + 4 + length {
                                assert!(headers.contains(if step == 0 {
                                    "trentservice.getpublickey"
                                } else {
                                    "trentservice.sign"
                                }));
                                break serde_json::from_slice::<serde_json::Value>(
                                    &raw[end + 4..end + 4 + length],
                                )
                                .unwrap();
                            }
                        }
                    };
                    assert_eq!(body["KeyId"], key_arn);
                    let response = if step == 0 {
                        serde_json::json!({
                            "KeyId":key_arn,"KeySpec":"ECC_NIST_EDWARDS25519","KeyUsage":"SIGN_VERIFY",
                            "SigningAlgorithms":["ED25519_SHA_512"],
                            "PublicKey":base64(&[ED25519_SPKI,&expected].concat())
                        })
                    } else {
                        assert_eq!(body["MessageType"], "RAW");
                        assert_eq!(body["SigningAlgorithm"], "ED25519_SHA_512");
                        assert_eq!(body["Message"], base64(message));
                        if mode == "denied" {
                            serde_json::json!({"__type":"AccessDeniedException","message":"private-provider-error-marker"})
                        } else {
                            let signed: &[u8] = if mode == "wrong_message" { b"different message" } else { message };
                            serde_json::json!({"KeyId":key_arn,"SigningAlgorithm":"ED25519_SHA_512","Signature":base64(&key.sign(signed).to_bytes())})
                        }
                    }.to_string();
                    let status = if step == 1 && mode == "denied" {
                        "400 Bad Request"
                    } else {
                        "200 OK"
                    };
                    write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/x-amz-json-1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
                }
            });
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let config = aws_sdk_kms::config::Builder::new()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("us-east-1"))
                .credentials_provider(aws_sdk_kms::config::Credentials::new(
                    "test",
                    "test",
                    None,
                    None,
                    "synthetic",
                ))
                .endpoint_url(endpoint)
                .retry_config(aws_config::retry::RetryConfig::standard().with_max_attempts(1))
                .build();
            let signer = AwsKmsSigner::from_client(
                runtime,
                Client::from_conf(config),
                key_arn.into(),
                expected,
            )
            .unwrap();
            let signed = signer.sign(message);
            if mode == "success" {
                assert!(checked_signature(&expected, message, &signed.unwrap()).is_ok());
            } else {
                assert!(
                    !signed
                        .unwrap_err()
                        .to_string()
                        .contains("private-provider-error-marker")
                );
            }
            server.join().unwrap();
        }
    }
}
