//! Deterministic account resolution and explicit dynamic-resolution ports.
//!
//! Static addresses and PDA recipes are generated from pinned IDLs. Anything
//! requiring live account data remains behind direct-RPC ports with no local or
//! indexer-backed implementation in this crate.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use curve25519_dalek::edwards::CompressedEdwardsY;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{InstructionContract, JobIntent, ProtocolLock, ResolvedAccountMeta};

const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";
const MAX_SEEDS: usize = 16;
const MAX_SEED_LENGTH: usize = 32;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountResolutionManifest {
    pub schema_version: u8,
    pub protocol_revision: String,
    pub programs: Vec<ResolutionProgram>,
    pub static_accounts: Vec<StaticAccountSpecification>,
    pub pda_recipes: Vec<PdaRecipe>,
}

impl AccountResolutionManifest {
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionProgram {
    pub name: String,
    pub program_id: String,
    pub idl_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StaticAccountSpecification {
    pub instruction_key: String,
    pub account_name: String,
    pub address: String,
    pub source: StaticAccountSource,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StaticAccountSource {
    IdlFixedAddress,
    InstructionProgram,
    ActionCpiProgram,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PdaRecipe {
    pub key: String,
    pub program: String,
    pub program_id: String,
    pub usages: Vec<PdaUsage>,
    pub seeds: Vec<PdaSeedRecipe>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PdaUsage {
    pub instruction_key: String,
    pub account_name: String,
    pub idl_seed_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PdaSeedRecipe {
    Const {
        #[serde(rename = "valueHex")]
        value_hex: String,
    },
    Input {
        name: String,
        encoding: PdaSeedEncoding,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PdaSeedEncoding {
    Pubkey,
    Bytes32,
    U64Le,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PdaSeedValue {
    Pubkey(String),
    Bytes32([u8; 32]),
    U64(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPda {
    pub address: String,
    pub bump: u8,
}

#[derive(Clone, Debug)]
pub struct DeterministicAccountResolver {
    manifest: AccountResolutionManifest,
}

impl DeterministicAccountResolver {
    pub fn new(
        lock: &ProtocolLock,
        contract: &InstructionContract,
        manifest: AccountResolutionManifest,
    ) -> Result<Self, AccountResolutionError> {
        if manifest.schema_version != 1 || manifest.protocol_revision != lock.revision {
            return Err(resolution_error(
                AccountResolutionErrorCode::ManifestMismatch,
                "account resolution manifest revision differs from protocol lock",
            ));
        }

        let mut program_names = BTreeSet::new();
        for program in &manifest.programs {
            if !program_names.insert(&program.name) {
                return Err(resolution_error(
                    AccountResolutionErrorCode::ManifestMismatch,
                    "account resolution manifest has a duplicate program",
                ));
            }
            let pinned = lock
                .programs
                .iter()
                .find(|entry| entry.name == program.name)
                .ok_or_else(|| {
                    resolution_error(
                        AccountResolutionErrorCode::ManifestMismatch,
                        "manifest program is absent from protocol lock",
                    )
                })?;
            if pinned.program_id.as_deref() != Some(&program.program_id)
                || pinned.idl.sha256.as_deref() != Some(&program.idl_sha256)
            {
                return Err(resolution_error(
                    AccountResolutionErrorCode::ManifestMismatch,
                    "manifest program provenance differs from protocol lock",
                ));
            }
        }

        let mut static_keys = BTreeSet::new();
        for account in &manifest.static_accounts {
            if !static_keys.insert((&account.instruction_key, &account.account_name)) {
                return Err(resolution_error(
                    AccountResolutionErrorCode::ManifestMismatch,
                    "manifest has a duplicate static account",
                ));
            }
            let specification = contract
                .instructions
                .iter()
                .find(|entry| entry.key == account.instruction_key)
                .ok_or_else(|| {
                    resolution_error(
                        AccountResolutionErrorCode::ManifestMismatch,
                        "static account instruction is absent from contract",
                    )
                })?;
            if !specification
                .accounts
                .iter()
                .any(|entry| entry.name == account.account_name)
                || decode_pubkey(&account.address).is_err()
            {
                return Err(resolution_error(
                    AccountResolutionErrorCode::ManifestMismatch,
                    "static account is not valid for its instruction",
                ));
            }
        }

        let mut recipe_keys = BTreeSet::new();
        for recipe in &manifest.pda_recipes {
            if !recipe_keys.insert(&recipe.key) || recipe.seeds.len() >= MAX_SEEDS {
                return Err(resolution_error(
                    AccountResolutionErrorCode::ManifestMismatch,
                    "manifest has a duplicate or oversized PDA recipe",
                ));
            }
            let program = manifest
                .programs
                .iter()
                .find(|entry| entry.name == recipe.program)
                .ok_or_else(|| {
                    resolution_error(
                        AccountResolutionErrorCode::ManifestMismatch,
                        "PDA recipe program is absent from manifest provenance",
                    )
                })?;
            if program.program_id != recipe.program_id || decode_pubkey(&recipe.program_id).is_err()
            {
                return Err(resolution_error(
                    AccountResolutionErrorCode::ManifestMismatch,
                    "PDA recipe program ID differs from manifest provenance",
                ));
            }
            let mut input_names = BTreeSet::new();
            for seed in &recipe.seeds {
                match seed {
                    PdaSeedRecipe::Const { value_hex } => {
                        let bytes = decode_hex(value_hex).map_err(|_| {
                            resolution_error(
                                AccountResolutionErrorCode::ManifestMismatch,
                                "PDA manifest contains invalid constant seed hex",
                            )
                        })?;
                        if bytes.is_empty() || bytes.len() > MAX_SEED_LENGTH {
                            return Err(resolution_error(
                                AccountResolutionErrorCode::ManifestMismatch,
                                "PDA manifest constant seed has invalid length",
                            ));
                        }
                    }
                    PdaSeedRecipe::Input { name, .. } if !input_names.insert(name) => {
                        return Err(resolution_error(
                            AccountResolutionErrorCode::ManifestMismatch,
                            "PDA manifest contains a duplicate input name",
                        ));
                    }
                    PdaSeedRecipe::Input { .. } => {}
                }
            }
            for usage in &recipe.usages {
                if !contract
                    .instructions
                    .iter()
                    .any(|entry| entry.key == usage.instruction_key)
                {
                    return Err(resolution_error(
                        AccountResolutionErrorCode::ManifestMismatch,
                        "PDA usage instruction is absent from contract",
                    ));
                }
            }
        }

        Ok(Self { manifest })
    }

    pub fn resolve_static(&self, instruction_key: &str, account_name: &str) -> Option<&str> {
        self.manifest
            .static_accounts
            .iter()
            .find(|entry| {
                entry.instruction_key == instruction_key && entry.account_name == account_name
            })
            .map(|entry| entry.address.as_str())
    }

    pub fn pda_recipe_key(&self, instruction_key: &str, account_name: &str) -> Option<&str> {
        self.manifest
            .pda_recipes
            .iter()
            .find(|recipe| {
                recipe.usages.iter().any(|usage| {
                    usage.instruction_key == instruction_key && usage.account_name == account_name
                })
            })
            .map(|recipe| recipe.key.as_str())
    }

    pub fn derive_pda(
        &self,
        recipe_key: &str,
        inputs: &BTreeMap<String, PdaSeedValue>,
    ) -> Result<ResolvedPda, AccountResolutionError> {
        let recipe = self
            .manifest
            .pda_recipes
            .iter()
            .find(|entry| entry.key == recipe_key)
            .ok_or_else(|| {
                resolution_error(
                    AccountResolutionErrorCode::UnknownRecipe,
                    format!("{recipe_key}: unknown PDA recipe"),
                )
            })?;
        let expected_names: BTreeSet<_> = recipe
            .seeds
            .iter()
            .filter_map(|seed| match seed {
                PdaSeedRecipe::Input { name, .. } => Some(name.as_str()),
                PdaSeedRecipe::Const { .. } => None,
            })
            .collect();
        if let Some(missing) = expected_names
            .iter()
            .find(|name| !inputs.contains_key(**name))
        {
            return Err(resolution_error(
                AccountResolutionErrorCode::MissingInput,
                format!("{recipe_key}: missing PDA input {missing}"),
            ));
        }
        if let Some(unexpected) = inputs
            .keys()
            .find(|name| !expected_names.contains(name.as_str()))
        {
            return Err(resolution_error(
                AccountResolutionErrorCode::UnexpectedInput,
                format!("{recipe_key}: unexpected PDA input {unexpected}"),
            ));
        }

        let mut encoded = Vec::with_capacity(recipe.seeds.len());
        for seed in &recipe.seeds {
            let bytes = match seed {
                PdaSeedRecipe::Const { value_hex } => decode_hex(value_hex).map_err(|_| {
                    resolution_error(
                        AccountResolutionErrorCode::ManifestMismatch,
                        "constant PDA seed is invalid",
                    )
                })?,
                PdaSeedRecipe::Input { name, encoding } => {
                    let value = inputs.get(name).expect("missing inputs were checked");
                    encode_seed_value(*encoding, value)?
                }
            };
            encoded.push(bytes);
        }
        find_program_address(&encoded, &recipe.program_id)
    }

    pub fn derive_pda_strings(
        &self,
        recipe_key: &str,
        inputs: &BTreeMap<String, String>,
    ) -> Result<ResolvedPda, AccountResolutionError> {
        let recipe = self
            .manifest
            .pda_recipes
            .iter()
            .find(|entry| entry.key == recipe_key)
            .ok_or_else(|| {
                resolution_error(
                    AccountResolutionErrorCode::UnknownRecipe,
                    format!("{recipe_key}: unknown PDA recipe"),
                )
            })?;
        let mut typed = BTreeMap::new();
        for seed in &recipe.seeds {
            let PdaSeedRecipe::Input { name, encoding } = seed else {
                continue;
            };
            let Some(value) = inputs.get(name) else {
                continue;
            };
            typed.insert(name.clone(), parse_seed_value(*encoding, value)?);
        }
        for (name, value) in inputs {
            if !typed.contains_key(name) {
                typed.insert(name.clone(), PdaSeedValue::Pubkey(value.clone()));
            }
        }
        self.derive_pda(recipe_key, &typed)
    }
}

fn parse_seed_value(
    encoding: PdaSeedEncoding,
    value: &str,
) -> Result<PdaSeedValue, AccountResolutionError> {
    match encoding {
        PdaSeedEncoding::Pubkey => decode_pubkey(value)
            .map(|_| PdaSeedValue::Pubkey(value.to_owned()))
            .map_err(|_| invalid_seed_value("PDA pubkey input must decode to 32 bytes")),
        PdaSeedEncoding::Bytes32 => decode_hex(value)
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .map(PdaSeedValue::Bytes32)
            .ok_or_else(|| invalid_seed_value("PDA bytes32 input must be 64 lowercase hex digits")),
        PdaSeedEncoding::U64Le => canonical_u64(value)
            .map(PdaSeedValue::U64)
            .ok_or_else(|| invalid_seed_value("PDA u64 input must be a canonical decimal string")),
    }
}

fn encode_seed_value(
    expected: PdaSeedEncoding,
    value: &PdaSeedValue,
) -> Result<Vec<u8>, AccountResolutionError> {
    match (expected, value) {
        (PdaSeedEncoding::Pubkey, PdaSeedValue::Pubkey(value)) => decode_pubkey(value)
            .map(|bytes| bytes.to_vec())
            .map_err(|_| invalid_seed_value("PDA pubkey input must decode to 32 bytes")),
        (PdaSeedEncoding::Bytes32, PdaSeedValue::Bytes32(value)) => Ok(value.to_vec()),
        (PdaSeedEncoding::U64Le, PdaSeedValue::U64(value)) => Ok(value.to_le_bytes().to_vec()),
        _ => Err(invalid_seed_value(
            "PDA input type differs from seed manifest",
        )),
    }
}

fn find_program_address(
    seeds: &[Vec<u8>],
    program_id: &str,
) -> Result<ResolvedPda, AccountResolutionError> {
    if seeds.len() >= MAX_SEEDS || seeds.iter().any(|seed| seed.len() > MAX_SEED_LENGTH) {
        return Err(invalid_seed_value(
            "PDA seed count or length exceeds Solana limits",
        ));
    }
    let program = decode_pubkey(program_id).map_err(|_| {
        resolution_error(
            AccountResolutionErrorCode::InvalidProgramId,
            "PDA program ID must decode to 32 bytes",
        )
    })?;

    for bump in (0..=u8::MAX).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([bump]);
        hasher.update(program);
        hasher.update(PDA_MARKER);
        let digest: [u8; 32] = hasher.finalize().into();
        if CompressedEdwardsY(digest).decompress().is_none() {
            return Ok(ResolvedPda {
                address: bs58::encode(digest).into_string(),
                bump,
            });
        }
    }
    Err(resolution_error(
        AccountResolutionErrorCode::UnableToFindPda,
        "no off-curve PDA exists for the supplied seeds",
    ))
}

fn decode_pubkey(value: &str) -> Result<[u8; 32], ()> {
    let bytes = bs58::decode(value).into_vec().map_err(|_| ())?;
    bytes.try_into().map_err(|_| ())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ()> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0]).ok_or(())?;
            let low = hex_nibble(pair[1]).ok_or(())?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn canonical_u64(value: &str) -> Option<u64> {
    if value.is_empty()
        || (value != "0" && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

fn invalid_seed_value(message: impl Into<String>) -> AccountResolutionError {
    resolution_error(AccountResolutionErrorCode::InvalidSeedValue, message)
}

fn resolution_error(
    code: AccountResolutionErrorCode,
    message: impl Into<String>,
) -> AccountResolutionError {
    AccountResolutionError {
        code,
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountResolutionErrorCode {
    ManifestMismatch,
    UnknownRecipe,
    MissingInput,
    UnexpectedInput,
    InvalidSeedValue,
    InvalidProgramId,
    UnableToFindPda,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountResolutionError {
    pub code: AccountResolutionErrorCode,
    message: String,
}

impl fmt::Display for AccountResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for AccountResolutionError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DynamicAccountRequirement {
    pub name: String,
    pub writable: bool,
    pub signer: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectRpcAccountRequest {
    pub intent: JobIntent,
    pub specification_key: String,
    pub unresolved_accounts: Vec<DynamicAccountRequirement>,
    pub minimum_context_slot: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectRpcAccountSnapshot {
    pub context_slot: u64,
    pub accounts: Vec<ResolvedAccountMeta>,
    pub state_hash: String,
}

/// Must fetch and decode dynamic accounts directly from RPC at or above the
/// requested context slot. Indexer data is never an implementation substitute.
pub trait DirectRpcDynamicAccountResolver: Send + Sync {
    type Error;
    fn resolve_and_revalidate(
        &self,
        request: &DirectRpcAccountRequest,
    ) -> Result<DirectRpcAccountSnapshot, Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token2022TransferLeg {
    pub leg_id: String,
    pub source: String,
    pub mint: String,
    pub destination: String,
    pub authority: String,
    pub amount: u64,
    pub decimals: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token2022RemainingAccountsRequest {
    pub intent: JobIntent,
    pub specification_key: String,
    pub transfer_legs: Vec<Token2022TransferLeg>,
    pub minimum_context_slot: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token2022RemainingAccountGroup {
    pub leg_id: String,
    pub accounts: Vec<ResolvedAccountMeta>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token2022RemainingAccountsSnapshot {
    pub context_slot: u64,
    pub groups: Vec<Token2022RemainingAccountGroup>,
}

/// Resolves transfer-hook extra metas from mint extensions and validation-state
/// accounts through direct RPC. The returned per-leg order must be preserved.
pub trait Token2022RemainingAccountResolver: Send + Sync {
    type Error;
    fn resolve_transfer_hook_accounts(
        &self,
        request: &Token2022RemainingAccountsRequest,
    ) -> Result<Token2022RemainingAccountsSnapshot, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = include_str!("../../../../protocol.lock.json");
    const CONTRACT: &str = include_str!("../../../../protocol/keeper-instructions.v1.json");
    const MANIFEST: &str = include_str!("../../../../protocol/keeper-account-resolution.v1.json");
    const FIXTURES: &str =
        include_str!("../../../../fixtures/conformance/v1/adapter-codec-cases.json");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureBundle {
        pda_cases: Vec<PdaCase>,
        invalid_pda_cases: Vec<InvalidPdaCase>,
        static_account_cases: Vec<StaticAccountCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PdaCase {
        name: String,
        recipe_key: String,
        inputs: BTreeMap<String, String>,
        expected_address: String,
        expected_bump: u8,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct InvalidPdaCase {
        name: String,
        recipe_key: String,
        inputs: BTreeMap<String, String>,
        expected_error: AccountResolutionErrorCode,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct StaticAccountCase {
        name: String,
        instruction_key: String,
        account_name: String,
        expected_address: String,
    }

    fn resolver() -> DeterministicAccountResolver {
        let lock = ProtocolLock::from_json(LOCK).expect("protocol lock must parse");
        let contract = InstructionContract::from_json(CONTRACT).expect("contract must parse");
        let manifest = AccountResolutionManifest::from_json(MANIFEST).expect("manifest must parse");
        DeterministicAccountResolver::new(&lock, &contract, manifest)
            .expect("manifest provenance must match")
    }

    #[test]
    fn derives_every_shared_pda_vector() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("adapter codec fixtures must parse");
        let resolver = resolver();
        for fixture in fixtures.pda_cases {
            let actual = resolver
                .derive_pda_strings(&fixture.recipe_key, &fixture.inputs)
                .unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
            assert_eq!(actual.address, fixture.expected_address, "{}", fixture.name);
            assert_eq!(actual.bump, fixture.expected_bump, "{}", fixture.name);
        }
    }

    #[test]
    fn rejects_every_shared_invalid_pda_vector() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("adapter codec fixtures must parse");
        let resolver = resolver();
        for fixture in fixtures.invalid_pda_cases {
            let error = resolver
                .derive_pda_strings(&fixture.recipe_key, &fixture.inputs)
                .unwrap_err();
            assert_eq!(error.code, fixture.expected_error, "{}", fixture.name);
        }
    }

    #[test]
    fn resolves_every_generated_static_account() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("adapter codec fixtures must parse");
        let resolver = resolver();
        for fixture in fixtures.static_account_cases {
            assert_eq!(
                resolver.resolve_static(&fixture.instruction_key, &fixture.account_name),
                Some(fixture.expected_address.as_str()),
                "{}",
                fixture.name
            );
        }
        assert_eq!(
            resolver.pda_recipe_key("dusk:trigger_liquidation_auction", "market"),
            Some("dusk:market")
        );
    }

    #[test]
    fn rejects_manifest_program_provenance_drift() {
        let lock = ProtocolLock::from_json(LOCK).expect("protocol lock must parse");
        let contract = InstructionContract::from_json(CONTRACT).expect("contract must parse");
        let mut manifest =
            AccountResolutionManifest::from_json(MANIFEST).expect("manifest must parse");
        manifest.programs[0].program_id = "11111111111111111111111111111111".into();
        let error = DeterministicAccountResolver::new(&lock, &contract, manifest).unwrap_err();
        assert_eq!(error.code, AccountResolutionErrorCode::ManifestMismatch);
    }
}
