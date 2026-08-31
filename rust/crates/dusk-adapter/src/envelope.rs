//! IDL-derived instruction envelope validation.
//!
//! This module validates resolved bytes and account metas. It deliberately does
//! not discover accounts, encode business arguments, sign, or submit.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ProtocolLock;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramName {
    Dusk,
    LeverageDelegate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobAction {
    LendingLiquidationTrigger,
    LendingLiquidationBid,
    LendingLiquidationFloorSettle,
    LeverageLiquidation,
    DelegatedCloseTakeProfit,
    DelegatedCloseStopLoss,
    ProtocolRevenueAuctionSettle,
    EligibleProposalQueue,
    EligibleProposalExecute,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    LeverageLiquidation,
    LendingLiquidationTrigger,
    LendingLiquidationBid,
    LendingLiquidationSettle,
    StopLoss,
    TakeProfit,
    AuctionBid,
    Lifecycle,
    Sentinel,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobIntent {
    pub schema_version: u8,
    pub intent_id: String,
    pub protocol_revision: String,
    pub action: JobAction,
    pub job_kind: JobKind,
    pub market: String,
    pub target: String,
    pub observed_slot: u64,
    pub expected_state_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAccountMeta {
    pub name: String,
    pub address: String,
    pub writable: bool,
    pub signer: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionEnvelope {
    pub specification_key: String,
    pub program: ProgramName,
    pub program_id: String,
    pub instruction_name: String,
    pub data_hex: String,
    pub accounts: Vec<ResolvedAccountMeta>,
    pub remaining_accounts: Vec<ResolvedAccountMeta>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEnvelope {
    pub schema_version: u8,
    pub intent: JobIntent,
    pub primary: InstructionEnvelope,
    pub cpi_hooks: Vec<InstructionEnvelope>,
    pub parity_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstructionContract {
    pub schema_version: u8,
    pub protocol_revision: String,
    pub programs: Vec<ProgramContract>,
    pub actions: Vec<ActionContract>,
    pub instructions: Vec<InstructionSpecification>,
}

impl InstructionContract {
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProgramContract {
    pub name: ProgramName,
    pub program_id: String,
    pub idl_path: String,
    pub idl_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActionContract {
    pub action: JobAction,
    pub job_kind: JobKind,
    pub primary_instruction_key: String,
    pub cpi_instruction_keys: Vec<String>,
    pub required_optional_accounts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstructionSpecification {
    pub key: String,
    pub program: ProgramName,
    pub program_id: String,
    pub instruction_name: String,
    pub discriminator_hex: String,
    pub accounts: Vec<AccountSpecification>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountSpecification {
    pub name: String,
    pub writable: bool,
    pub signer: bool,
    pub optional: bool,
    pub fixed_address: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ValidatedJobEnvelope(JobEnvelope);

impl ValidatedJobEnvelope {
    pub fn envelope(&self) -> &JobEnvelope {
        &self.0
    }
}

/// A validated envelope whose protocol lock has also passed the complete live
/// readiness gate. Only this type may cross the signing port.
#[derive(Clone, Debug)]
pub struct SignableJobEnvelope(ValidatedJobEnvelope);

impl SignableJobEnvelope {
    pub fn envelope(&self) -> &JobEnvelope {
        self.0.envelope()
    }
}

#[derive(Clone, Debug)]
pub struct EnvelopeValidator {
    lock: ProtocolLock,
    contract: InstructionContract,
}

impl EnvelopeValidator {
    pub fn new(
        lock: ProtocolLock,
        contract: InstructionContract,
    ) -> Result<Self, EnvelopeValidationError> {
        if contract.schema_version != 1 || contract.protocol_revision != lock.revision {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::RevisionMismatch,
                "instruction contract revision does not match protocol lock",
            ));
        }

        let mut names = BTreeSet::new();
        for program in &contract.programs {
            if !names.insert(program.name as u8) {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::InstructionMismatch,
                    "instruction contract contains a duplicate program",
                ));
            }
            let expected_name = match program.name {
                ProgramName::Dusk => "dusk",
                ProgramName::LeverageDelegate => "leverage_delegate",
            };
            let Some(pinned) = lock
                .programs
                .iter()
                .find(|entry| entry.name == expected_name)
            else {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::ProgramIdMismatch,
                    "instruction contract program is absent from protocol lock",
                ));
            };
            if pinned.program_id.as_deref() != Some(&program.program_id)
                || pinned.idl.path != program.idl_path
                || pinned.idl.sha256.as_deref() != Some(&program.idl_sha256)
            {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::ProgramIdMismatch,
                    "instruction contract program provenance differs from protocol lock",
                ));
            }
        }
        Ok(Self { lock, contract })
    }

    pub fn validate(
        &self,
        envelope: JobEnvelope,
    ) -> Result<ValidatedJobEnvelope, EnvelopeValidationError> {
        if envelope.schema_version != 1 || envelope.intent.schema_version != 1 {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::InstructionMismatch,
                "unsupported envelope schema",
            ));
        }
        if envelope.intent.protocol_revision != self.lock.revision {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::RevisionMismatch,
                "job intent revision does not match protocol lock",
            ));
        }
        let action = self
            .contract
            .actions
            .iter()
            .find(|candidate| candidate.action == envelope.intent.action)
            .ok_or_else(|| {
                EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::InstructionMismatch,
                    "job action is absent from instruction contract",
                )
            })?;
        if envelope.intent.job_kind != action.job_kind {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::JobKindMismatch,
                "job kind does not match action",
            ));
        }

        self.validate_expected_instruction(&envelope.primary, &action.primary_instruction_key)?;
        if envelope.cpi_hooks.len() != action.cpi_instruction_keys.len()
            || envelope
                .cpi_hooks
                .iter()
                .zip(&action.cpi_instruction_keys)
                .any(|(actual, expected)| actual.specification_key != *expected)
        {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::CpiHookMismatch,
                "CPI hook sequence does not match job action",
            ));
        }
        for (hook, expected) in envelope.cpi_hooks.iter().zip(&action.cpi_instruction_keys) {
            self.validate_expected_instruction(hook, expected)?;
            if !hook.remaining_accounts.is_empty() {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::CpiHookMismatch,
                    "nested CPI hook remaining accounts are not supported",
                ));
            }
        }

        for required in &action.required_optional_accounts {
            if !envelope
                .primary
                .accounts
                .iter()
                .any(|account| account.name == *required)
            {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::RequiredOptionalAccountMissing,
                    format!("job action requires optional IDL account {required}"),
                ));
            }
        }

        if !action.cpi_instruction_keys.is_empty() {
            self.validate_delegated_composition(&envelope)?;
        }

        let actual_parity = envelope_parity_sha256(&envelope)?;
        if actual_parity != envelope.parity_sha256 {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::ParityMismatch,
                "canonical byte/account-order digest differs",
            ));
        }
        Ok(ValidatedJobEnvelope(envelope))
    }

    /// Applies the frozen-lock/live-provenance gate before returning a value
    /// accepted by `EnvelopeSigner`.
    pub fn validate_for_signing(
        &self,
        envelope: JobEnvelope,
    ) -> Result<SignableJobEnvelope, EnvelopeValidationError> {
        self.lock.assert_live_ready().map_err(|error| {
            EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::LiveProtocolNotReady,
                format!("protocol lock is not ready for signing: {error}"),
            )
        })?;
        self.validate(envelope).map(SignableJobEnvelope)
    }

    fn validate_expected_instruction(
        &self,
        envelope: &InstructionEnvelope,
        expected_key: &str,
    ) -> Result<(), EnvelopeValidationError> {
        if envelope.specification_key != expected_key {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::InstructionMismatch,
                "instruction specification key differs",
            ));
        }
        let specification = self
            .contract
            .instructions
            .iter()
            .find(|candidate| candidate.key == expected_key)
            .ok_or_else(|| {
                EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::InstructionMismatch,
                    "instruction specification is absent",
                )
            })?;
        if envelope.program != specification.program
            || envelope.instruction_name != specification.instruction_name
        {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::InstructionMismatch,
                "program or instruction name differs from IDL contract",
            ));
        }
        if envelope.program_id != specification.program_id {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::ProgramIdMismatch,
                "program ID differs from pinned contract",
            ));
        }
        require_pubkey(&envelope.program_id)?;
        let data = decode_hex(&envelope.data_hex)?;
        let discriminator = decode_hex(&specification.discriminator_hex)?;
        if !data.starts_with(&discriminator) {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::DiscriminatorMismatch,
                "instruction data does not start with the pinned Anchor discriminator",
            ));
        }
        self.validate_accounts(envelope, specification)?;
        for account in &envelope.remaining_accounts {
            require_pubkey(&account.address)?;
        }
        Ok(())
    }

    fn validate_accounts(
        &self,
        envelope: &InstructionEnvelope,
        specification: &InstructionSpecification,
    ) -> Result<(), EnvelopeValidationError> {
        let mut actual_index = 0;
        for expected in &specification.accounts {
            let actual = envelope.accounts.get(actual_index);
            if expected.optional && actual.is_none_or(|account| account.name != expected.name) {
                continue;
            }
            let Some(actual) = actual else {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::MissingRequiredAccount,
                    format!("missing required account {}", expected.name),
                ));
            };
            if actual.name != expected.name {
                let expected_is_later = envelope.accounts[actual_index + 1..]
                    .iter()
                    .any(|account| account.name == expected.name);
                let code = if expected_is_later {
                    EnvelopeValidationErrorCode::AccountOrderMismatch
                } else {
                    EnvelopeValidationErrorCode::MissingRequiredAccount
                };
                return Err(EnvelopeValidationError::new(
                    code,
                    format!("expected account {}, got {}", expected.name, actual.name),
                ));
            }
            require_pubkey(&actual.address)?;
            if actual.writable != expected.writable || actual.signer != expected.signer {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::AccountFlagsMismatch,
                    format!("account {} flags differ from IDL", expected.name),
                ));
            }
            let fixed_address = expected.fixed_address.as_deref().or_else(|| {
                (expected.name == "program").then_some(specification.program_id.as_str())
            });
            if fixed_address.is_some_and(|address| address != actual.address) {
                return Err(EnvelopeValidationError::new(
                    EnvelopeValidationErrorCode::FixedAddressMismatch,
                    format!("account {} differs from its fixed address", expected.name),
                ));
            }
            actual_index += 1;
        }
        if actual_index != envelope.accounts.len() {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::AccountOrderMismatch,
                "instruction envelope has unexpected base accounts",
            ));
        }
        Ok(())
    }

    fn validate_delegated_composition(
        &self,
        envelope: &JobEnvelope,
    ) -> Result<(), EnvelopeValidationError> {
        if envelope.cpi_hooks.len() != 2 {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::CpiHookMismatch,
                "delegated close requires exactly two CPI hooks",
            ));
        }
        let primary_data = decode_hex(&envelope.primary.data_hex)?;
        let before_data = decode_hex(&envelope.cpi_hooks[0].data_hex)?;
        let after_data = decode_hex(&envelope.cpi_hooks[1].data_hex)?;
        let (embedded_before, embedded_after, before_accounts_len) =
            decode_delegated_cpi_payload(&primary_data)?;
        if embedded_before != before_data || embedded_after != after_data {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::DelegatedHookDataMismatch,
                "delegated close embedded CPI bytes differ from hook envelopes",
            ));
        }
        if before_accounts_len != envelope.cpi_hooks[0].accounts.len() {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::RemainingAccountsMismatch,
                "before_accounts_len differs from before-hook account count",
            ));
        }
        let expected_remaining: Vec<_> = envelope
            .cpi_hooks
            .iter()
            .flat_map(|hook| hook.accounts.iter().cloned())
            .collect();
        if envelope.primary.remaining_accounts != expected_remaining {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::RemainingAccountsMismatch,
                "primary remaining accounts are not the exact before/after hook concatenation",
            ));
        }

        let delegated_program = envelope
            .primary
            .accounts
            .iter()
            .find(|account| account.name == "delegated_program")
            .expect("required optional account was checked");
        if delegated_program.address != envelope.cpi_hooks[0].program_id {
            return Err(EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::FixedAddressMismatch,
                "delegated_program does not match CPI hook program",
            ));
        }
        Ok(())
    }
}

fn decode_delegated_cpi_payload(
    data: &[u8],
) -> Result<(Vec<u8>, Vec<u8>, usize), EnvelopeValidationError> {
    if data.len() < 8 + 1 + 8 + 4 + 4 + 2 {
        return Err(invalid_delegated_data());
    }
    let mut cursor = 8 + 1 + 8;
    let before = read_vector(data, &mut cursor)?;
    let after = read_vector(data, &mut cursor)?;
    let end = cursor.checked_add(2).ok_or_else(invalid_delegated_data)?;
    if end != data.len() {
        return Err(invalid_delegated_data());
    }
    let count = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
    Ok((before, after, count))
}

fn read_vector(data: &[u8], cursor: &mut usize) -> Result<Vec<u8>, EnvelopeValidationError> {
    let length_end = cursor.checked_add(4).ok_or_else(invalid_delegated_data)?;
    let length_bytes: [u8; 4] = data
        .get(*cursor..length_end)
        .ok_or_else(invalid_delegated_data)?
        .try_into()
        .map_err(|_| invalid_delegated_data())?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    *cursor = length_end;
    let end = cursor
        .checked_add(length)
        .ok_or_else(invalid_delegated_data)?;
    let bytes = data
        .get(*cursor..end)
        .ok_or_else(invalid_delegated_data)?
        .to_vec();
    *cursor = end;
    Ok(bytes)
}

fn invalid_delegated_data() -> EnvelopeValidationError {
    EnvelopeValidationError::new(
        EnvelopeValidationErrorCode::DelegatedHookDataMismatch,
        "delegated close CPI payload is malformed",
    )
}

fn require_pubkey(address: &str) -> Result<(), EnvelopeValidationError> {
    let decoded = bs58::decode(address).into_vec().map_err(|_| {
        EnvelopeValidationError::new(
            EnvelopeValidationErrorCode::InvalidPubkey,
            "account address is not base58",
        )
    })?;
    if decoded.len() != 32 {
        return Err(EnvelopeValidationError::new(
            EnvelopeValidationErrorCode::InvalidPubkey,
            "account address does not decode to 32 bytes",
        ));
    }
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, EnvelopeValidationError> {
    if value.len() < 16 || !value.len().is_multiple_of(2) {
        return Err(invalid_hex());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = decode_nibble(pair[0]).ok_or_else(invalid_hex)?;
            let low = decode_nibble(pair[1]).ok_or_else(invalid_hex)?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn invalid_hex() -> EnvelopeValidationError {
    EnvelopeValidationError::new(
        EnvelopeValidationErrorCode::InvalidDataHex,
        "instruction data must be lowercase, even-length hex with an 8-byte discriminator",
    )
}

fn envelope_parity_sha256(envelope: &JobEnvelope) -> Result<String, EnvelopeValidationError> {
    let mut writer = CanonicalWriter::default();
    writer.u8(envelope.schema_version);
    writer.u8(envelope.intent.schema_version);
    writer.string(&envelope.intent.intent_id)?;
    writer.string(&envelope.intent.protocol_revision)?;
    writer.string(&snake_case(envelope.intent.action))?;
    writer.string(&snake_case(envelope.intent.job_kind))?;
    writer.string(&envelope.intent.market)?;
    writer.string(&envelope.intent.target)?;
    writer.u64(envelope.intent.observed_slot);
    writer.string(&envelope.intent.expected_state_hash)?;
    writer.instruction(&envelope.primary)?;
    writer.u32(envelope.cpi_hooks.len())?;
    for hook in &envelope.cpi_hooks {
        writer.instruction(hook)?;
    }
    let digest = Sha256::digest(writer.finish());
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn snake_case<T: Serialize>(value: T) -> String {
    serde_json::to_string(&value)
        .expect("enum serialization cannot fail")
        .trim_matches('"')
        .to_owned()
}

#[derive(Default)]
struct CanonicalWriter {
    bytes: Vec<u8>,
}

impl CanonicalWriter {
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u32(&mut self, value: usize) -> Result<(), EnvelopeValidationError> {
        let value = u32::try_from(value).map_err(|_| {
            EnvelopeValidationError::new(
                EnvelopeValidationErrorCode::ParityMismatch,
                "canonical field exceeds u32 length",
            )
        })?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn string(&mut self, value: &str) -> Result<(), EnvelopeValidationError> {
        self.u32(value.len())?;
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn byte_slice(&mut self, value: &[u8]) -> Result<(), EnvelopeValidationError> {
        self.u32(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn account(&mut self, account: &ResolvedAccountMeta) -> Result<(), EnvelopeValidationError> {
        self.string(&account.name)?;
        self.string(&account.address)?;
        self.u8(u8::from(account.writable));
        self.u8(u8::from(account.signer));
        Ok(())
    }

    fn instruction(
        &mut self,
        instruction: &InstructionEnvelope,
    ) -> Result<(), EnvelopeValidationError> {
        self.string(&instruction.specification_key)?;
        self.string(&snake_case(instruction.program))?;
        self.string(&instruction.program_id)?;
        self.string(&instruction.instruction_name)?;
        self.byte_slice(&decode_hex(&instruction.data_hex)?)?;
        self.u32(instruction.accounts.len())?;
        for account in &instruction.accounts {
            self.account(account)?;
        }
        self.u32(instruction.remaining_accounts.len())?;
        for account in &instruction.remaining_accounts {
            self.account(account)?;
        }
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeValidationErrorCode {
    RevisionMismatch,
    JobKindMismatch,
    InstructionMismatch,
    ProgramIdMismatch,
    InvalidPubkey,
    InvalidDataHex,
    DiscriminatorMismatch,
    MissingRequiredAccount,
    AccountOrderMismatch,
    AccountFlagsMismatch,
    FixedAddressMismatch,
    RequiredOptionalAccountMissing,
    CpiHookMismatch,
    RemainingAccountsMismatch,
    DelegatedHookDataMismatch,
    ParityMismatch,
    LiveProtocolNotReady,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvelopeValidationError {
    pub code: EnvelopeValidationErrorCode,
    message: String,
}

impl EnvelopeValidationError {
    fn new(code: EnvelopeValidationErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for EnvelopeValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for EnvelopeValidationError {}

/// The signer accepts only envelopes validated under a live-ready frozen lock;
/// implementations remain outside this crate and should enforce service-wallet
/// policies remotely.
pub trait EnvelopeSigner: Send + Sync {
    type Error;
    type SignedTransaction;
    fn sign(&self, envelope: &SignableJobEnvelope) -> Result<Self::SignedTransaction, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = include_str!("../../../../protocol.lock.json");
    const CONTRACT: &str = include_str!("../../../../protocol/keeper-instructions.v1.json");
    const FIXTURES: &str =
        include_str!("../../../../fixtures/conformance/v1/instruction-envelope-cases.json");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureBundle {
        valid_cases: Vec<ValidCase>,
        invalid_cases: Vec<InvalidCase>,
    }

    #[derive(Deserialize)]
    struct ValidCase {
        name: String,
        envelope: JobEnvelope,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct InvalidCase {
        name: String,
        expected_error: EnvelopeValidationErrorCode,
        envelope: JobEnvelope,
    }

    fn validator() -> EnvelopeValidator {
        let lock = ProtocolLock::from_json(LOCK).expect("captured lock must parse");
        let contract =
            InstructionContract::from_json(CONTRACT).expect("instruction contract must parse");
        EnvelopeValidator::new(lock, contract).expect("contract must match lock")
    }

    /// Rewrites every valid case's parity digest in place.
    ///
    /// The digest is a checksum over the envelope's bytes and account order,
    /// not an independent statement about the protocol, so recomputing it with
    /// the production function is the correct way to re-pin these fixtures
    /// after a deployment change. Ignored by default; run explicitly with
    /// `cargo test -p dusk-adapter -- --ignored regenerate_parity`.
    #[test]
    #[ignore]
    fn regenerate_parity_digests() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/conformance/v1/instruction-envelope-cases.json"
        );
        let raw = std::fs::read_to_string(path).expect("fixtures must be readable");
        let mut document: serde_json::Value =
            serde_json::from_str(&raw).expect("fixtures must parse");

        let cases = document
            .get_mut("validCases")
            .and_then(|value| value.as_array_mut())
            .expect("validCases must be an array");
        for case in cases {
            let envelope_value = case.get("envelope").expect("case must carry an envelope");
            let envelope: JobEnvelope = serde_json::from_value(envelope_value.clone())
                .expect("envelope must parse");
            let parity = envelope_parity_sha256(&envelope).expect("parity must compute");
            case["envelope"]["paritySha256"] = serde_json::Value::String(parity);
        }

        let rendered = serde_json::to_string_pretty(&document).expect("fixtures must render");
        std::fs::write(path, format!("{rendered}\n")).expect("fixtures must be writable");
    }

    #[test]
    fn validates_all_shared_real_instruction_envelopes() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("instruction fixtures must parse");
        let validator = validator();
        for fixture in fixtures.valid_cases {
            validator
                .validate(fixture.envelope)
                .unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
        }
    }

    #[test]
    fn rejects_all_shared_invalid_instruction_envelopes() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("instruction fixtures must parse");
        let validator = validator();
        for fixture in fixtures.invalid_cases {
            let error = validator.validate(fixture.envelope).unwrap_err();
            assert_eq!(error.code, fixture.expected_error, "{}", fixture.name);
        }
    }

    #[test]
    fn captured_lock_cannot_produce_a_signable_envelope() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("instruction fixtures must parse");
        let envelope = fixtures
            .valid_cases
            .into_iter()
            .next()
            .expect("a valid fixture must exist")
            .envelope;

        let error = validator().validate_for_signing(envelope).unwrap_err();
        assert_eq!(
            error.code,
            EnvelopeValidationErrorCode::LiveProtocolNotReady
        );
    }
}
