//! Pinned Dusk protocol boundary.
//!
//! This crate must stay independent from keeper scheduling. It owns protocol
//! revision validation, account decoding, PDA derivation, and instruction
//! construction once those generated implementations are added.

use std::{error::Error, fmt, fs, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolLock {
    pub schema_version: u8,
    pub revision: String,
    pub status: LockStatus,
    pub generated_at: Option<String>,
    pub source: SourceRevision,
    pub toolchain: Toolchain,
    pub programs: Vec<PinnedProgram>,
    pub sdk: Artifact,
    pub compatibility: CompatibilityFingerprints,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LockStatus {
    Draft,
    Captured,
    Frozen,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRevision {
    pub repository: String,
    pub branch: String,
    pub git_commit: String,
    pub worktree_fingerprint_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Toolchain {
    pub anchor: Option<String>,
    pub rust: Option<String>,
    pub solana: Option<String>,
    pub surfpool: Option<String>,
    pub node: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PinnedProgram {
    pub name: String,
    pub program_id: Option<String>,
    pub binary: Artifact,
    pub idl: Artifact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Artifact {
    pub path: String,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityFingerprints {
    pub account_layout_fingerprint_sha256: Option<String>,
    pub instruction_fingerprint_sha256: Option<String>,
    pub event_fingerprint_sha256: Option<String>,
    pub error_fingerprint_sha256: Option<String>,
    pub pda_fingerprint_sha256: Option<String>,
}

impl ProtocolLock {
    pub fn from_json(raw: &str) -> Result<Self, ProtocolLockError> {
        serde_json::from_str(raw).map_err(ProtocolLockError::InvalidJson)
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self, ProtocolLockError> {
        let raw = fs::read_to_string(path).map_err(ProtocolLockError::Read)?;
        Self::from_json(&raw)
    }

    /// Rejects every incomplete pin before a signer or RPC sender is created.
    pub fn assert_live_ready(&self) -> Result<(), ProtocolLockError> {
        if self.schema_version != 1 {
            return Err(ProtocolLockError::UnsupportedSchema(self.schema_version));
        }
        require_present(Some(&self.revision), "revision")?;
        if self.status != LockStatus::Frozen {
            return Err(ProtocolLockError::NotFrozen);
        }
        require_present(self.generated_at.as_deref(), "generatedAt")?;
        require_hash(
            self.source.worktree_fingerprint_sha256.as_deref(),
            "source.worktreeFingerprintSha256",
        )?;

        for (name, value) in [
            ("toolchain.anchor", self.toolchain.anchor.as_deref()),
            ("toolchain.rust", self.toolchain.rust.as_deref()),
            ("toolchain.solana", self.toolchain.solana.as_deref()),
            ("toolchain.surfpool", self.toolchain.surfpool.as_deref()),
            ("toolchain.node", self.toolchain.node.as_deref()),
        ] {
            require_present(value, name)?;
        }

        if self.programs.is_empty() {
            return Err(ProtocolLockError::MissingField("programs"));
        }
        for program in &self.programs {
            require_present(program.program_id.as_deref(), "programs[].programId")?;
            require_hash(program.binary.sha256.as_deref(), "programs[].binary.sha256")?;
            require_hash(program.idl.sha256.as_deref(), "programs[].idl.sha256")?;
        }
        require_hash(self.sdk.sha256.as_deref(), "sdk.sha256")?;
        for (name, value) in [
            (
                "compatibility.accountLayoutFingerprintSha256",
                self.compatibility
                    .account_layout_fingerprint_sha256
                    .as_deref(),
            ),
            (
                "compatibility.instructionFingerprintSha256",
                self.compatibility.instruction_fingerprint_sha256.as_deref(),
            ),
            (
                "compatibility.eventFingerprintSha256",
                self.compatibility.event_fingerprint_sha256.as_deref(),
            ),
            (
                "compatibility.errorFingerprintSha256",
                self.compatibility.error_fingerprint_sha256.as_deref(),
            ),
            (
                "compatibility.pdaFingerprintSha256",
                self.compatibility.pda_fingerprint_sha256.as_deref(),
            ),
        ] {
            require_hash(value, name)?;
        }
        Ok(())
    }
}

fn require_present(value: Option<&str>, name: &'static str) -> Result<(), ProtocolLockError> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(()),
        _ => Err(ProtocolLockError::MissingField(name)),
    }
}

fn require_hash(value: Option<&str>, name: &'static str) -> Result<(), ProtocolLockError> {
    let Some(value) = value else {
        return Err(ProtocolLockError::MissingField(name));
    };
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(ProtocolLockError::InvalidHash(name))
    }
}

#[derive(Debug)]
pub enum ProtocolLockError {
    Read(std::io::Error),
    InvalidJson(serde_json::Error),
    UnsupportedSchema(u8),
    NotFrozen,
    MissingField(&'static str),
    InvalidHash(&'static str),
}

impl fmt::Display for ProtocolLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "failed to read protocol lock: {error}"),
            Self::InvalidJson(error) => write!(formatter, "invalid protocol lock JSON: {error}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported protocol lock schema {version}")
            }
            Self::NotFrozen => write!(formatter, "protocol lock is not frozen"),
            Self::MissingField(field) => write!(formatter, "protocol lock is missing {field}"),
            Self::InvalidHash(field) => write!(formatter, "protocol lock has an invalid {field}"),
        }
    }
}

impl Error for ProtocolLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::InvalidJson(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildRequest {
    pub job_kind: String,
    pub market: String,
    pub target: String,
    pub expected_state_hash: String,
    pub observed_slot: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionIntent {
    pub protocol_revision: String,
    pub program_id: String,
    pub accounts: Vec<AccountMeta>,
    pub data_hex: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMeta {
    pub address: String,
    pub writable: bool,
    pub signer: bool,
}

pub trait DuskAdapter: Send + Sync {
    fn protocol_revision(&self) -> &str;
    fn build_instruction(&self, request: &BuildRequest) -> Result<InstructionIntent, AdapterError>;
}

#[derive(Debug, Eq, PartialEq)]
pub enum AdapterError {
    RevisionMismatch,
    StateChanged,
    UnsupportedJob,
    NotImplemented,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for AdapterError {}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURED_LOCK: &str = include_str!("../../../../protocol.lock.json");

    #[test]
    fn parses_the_shared_lock() {
        let lock = ProtocolLock::from_json(CAPTURED_LOCK).expect("captured lock must parse");
        assert_eq!(lock.revision, "local-snapshot-0");
        assert_eq!(lock.status, LockStatus::Captured);
        assert_eq!(lock.programs.len(), 2);
    }

    #[test]
    fn captured_lock_cannot_enable_live_execution() {
        let lock = ProtocolLock::from_json(CAPTURED_LOCK).expect("captured lock must parse");
        assert!(matches!(
            lock.assert_live_ready(),
            Err(ProtocolLockError::NotFrozen)
        ));
    }
}
