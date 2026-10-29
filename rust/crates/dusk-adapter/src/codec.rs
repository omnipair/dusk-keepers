//! Native Borsh argument encoding for the keeper-critical instruction set.
//!
//! Decimal strings are used only at the JSON boundary so every u64 value is
//! lossless in both runtimes. Native callers use the strongly typed argument
//! structures below.

use std::{error::Error, fmt};

use borsh::BorshSerialize;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::InstructionContract;

#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct ObserveLiquidationArgs {
    pub debt_asset: u8,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObserveLiquidationWire {
    debt_asset: u8,
}
#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct PreviewFlashLiquidationArgs {
    pub debt_asset: u8,
    pub max_repayment: u64,
    pub full: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewFlashLiquidationWire {
    debt_asset: u8,
    max_repayment: String,
    full: bool,
}
#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct PreviewEmergencyLiquidationArgs {
    pub debt_asset: u8,
    pub collateral_debit: u64,
    pub full: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewEmergencyLiquidationWire {
    debt_asset: u8,
    collateral_debit: String,
    full: bool,
}
#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct BeginFlashLiquidationArgs {
    pub position: [u8; 32],
    pub debt_asset: u8,
    pub max_repayment: u64,
    pub max_payment: u64,
    pub min_collateral_credit: u64,
    pub min_buyer_credit: u64,
    pub full: bool,
    pub settle_index: u16,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BeginFlashLiquidationWire {
    position: String,
    debt_asset: u8,
    max_repayment: String,
    max_payment: String,
    min_collateral_credit: String,
    min_buyer_credit: String,
    full: bool,
    settle_index: u16,
}
#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct EmergencyLiquidationArgs {
    pub debt_asset: u8,
    pub collateral_debit: u64,
    pub full: bool,
    pub min_reward_credit: u64,
    pub min_swap_output: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmergencyLiquidationWire {
    debt_asset: u8,
    collateral_debit: String,
    full: bool,
    min_reward_credit: String,
    min_swap_output: String,
}

#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct PreviewHlpFlashArgs {
    pub target_asset: u8,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewHlpFlashWire {
    target_asset: u8,
}
#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct BeginHlpFlashArgs {
    pub target_asset: u8,
    pub max_payment: u64,
    pub allow_full: bool,
    pub max_insurance_draw: u64,
    pub max_socialized_loss: u64,
    pub max_collateral_debit: u64,
    pub min_collateral_credit: u64,
    pub settle_index: u16,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BeginHlpFlashWire {
    target_asset: u8,
    max_payment: String,
    allow_full: bool,
    max_insurance_draw: String,
    max_socialized_loss: String,
    max_collateral_debit: String,
    min_collateral_credit: String,
    settle_index: u16,
}
#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct CloseInsolventHlpArgs {
    pub target_asset: u8,
    pub max_insurance_draw: u64,
    pub max_socialized_loss: u64,
    pub min_caller_bounty_out: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CloseInsolventHlpWire {
    target_asset: u8,
    max_insurance_draw: String,
    max_socialized_loss: String,
    min_caller_bounty_out: String,
}

#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct DelegatedCpiArgs {
    pub before_ix_data: Vec<u8>,
    pub after_ix_data: Vec<u8>,
    pub before_accounts_len: u16,
}

#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct DelegatedCloseLeverageArgs {
    pub debt_asset: u8,
    pub min_amount_out: u64,
    /// What fraction of the position to close. Field order is the borsh wire
    /// order, so this sits between the minimum out and the CPI payload.
    pub close_bps: u16,
    pub delegated: DelegatedCpiArgs,
}

#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct ExecuteOrderArgs {
    pub order_id: u64,
}

#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct ExecuteEntryOrderArgs {
    pub order_id: u64,
    pub min_owner_refund_out: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecuteEntryOrderWire {
    order_id: String,
    min_owner_refund_out: String,
}

#[derive(Clone, Copy, Debug, BorshSerialize, Eq, PartialEq)]
pub enum ProtocolAuctionLane {
    Fee,
    Buyback,
}

#[derive(Clone, Copy, Debug, BorshSerialize, Eq, PartialEq)]
pub enum ProtocolRevenueSource {
    Swap,
    Interest,
}

#[derive(Clone, Debug, BorshSerialize, Eq, PartialEq)]
pub struct SettleProtocolAuctionArgs {
    pub lane: ProtocolAuctionLane,
    pub source: ProtocolRevenueSource,
    pub sold_amount: u64,
    pub max_payment_amount: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeeperInstructionArguments {
    ObserveLiquidation(ObserveLiquidationArgs),
    PreviewFlashLiquidation(PreviewFlashLiquidationArgs),
    PreviewEmergencyLiquidation(PreviewEmergencyLiquidationArgs),
    BeginFlashLiquidation(BeginFlashLiquidationArgs),
    EmergencyLiquidation(EmergencyLiquidationArgs),
    SettleFlashLiquidation,
    PreviewHlpFlash(PreviewHlpFlashArgs),
    BeginHlpFlash(BeginHlpFlashArgs),
    SettleHlpFlash,
    CloseInsolventHlp(CloseInsolventHlpArgs),

    DelegatedCloseLeverage(DelegatedCloseLeverageArgs),
    BeforeTakeProfit(ExecuteOrderArgs),
    AfterCloseOrder(ExecuteOrderArgs),
    BeforeStopLoss(ExecuteOrderArgs),
    ExecuteHlpOrder(ExecuteOrderArgs),
    ExecuteEntryOrder(ExecuteEntryOrderArgs),
    SettleProtocolAuction(SettleProtocolAuctionArgs),
    QueueParameterProposal,
    ExecuteParameterProposal,
}

impl KeeperInstructionArguments {
    pub fn specification_key(&self) -> &'static str {
        match self {
            Self::ObserveLiquidation(_) => "dusk:observe_liquidation",
            Self::PreviewFlashLiquidation(_) => "dusk:preview_flash_liquidation",
            Self::PreviewEmergencyLiquidation(_) => "dusk:preview_emergency_liquidation",
            Self::BeginFlashLiquidation(_) => "dusk:begin_flash_liquidation",
            Self::EmergencyLiquidation(_) => "dusk:emergency_liquidation",
            Self::SettleFlashLiquidation => "dusk:settle_flash_liquidation",
            Self::PreviewHlpFlash(_) => "dusk:preview_hlp_flash",
            Self::BeginHlpFlash(_) => "dusk:begin_hlp_flash",
            Self::SettleHlpFlash => "dusk:settle_hlp_flash",
            Self::CloseInsolventHlp(_) => "dusk:close_insolvent_hlp",

            Self::DelegatedCloseLeverage(_) => "dusk:delegated_close_leverage",
            Self::BeforeTakeProfit(_) => "leverage_delegate:before_take_profit",
            Self::AfterCloseOrder(_) => "leverage_delegate:after_close_order",
            Self::BeforeStopLoss(_) => "leverage_delegate:before_stop_loss",
            Self::ExecuteHlpOrder(_) => "leverage_delegate:execute_hlp_order",
            Self::ExecuteEntryOrder(_) => "leverage_delegate:execute_leverage_entry_order",
            Self::SettleProtocolAuction(_) => "dusk:settle_protocol_auction",
            Self::QueueParameterProposal => "dusk:queue_parameter_proposal",
            Self::ExecuteParameterProposal => "dusk:execute_parameter_proposal",
        }
    }

    fn borsh_bytes(&self) -> Result<Vec<u8>, InstructionEncodingError> {
        let result = match self {
            Self::ObserveLiquidation(value) => borsh::to_vec(value),
            Self::PreviewFlashLiquidation(value) => borsh::to_vec(value),
            Self::PreviewEmergencyLiquidation(value) => borsh::to_vec(value),
            Self::BeginFlashLiquidation(value) => borsh::to_vec(value),
            Self::EmergencyLiquidation(value) => borsh::to_vec(value),
            Self::SettleFlashLiquidation => return Ok(Vec::new()),
            Self::PreviewHlpFlash(value) => borsh::to_vec(value),
            Self::BeginHlpFlash(value) => borsh::to_vec(value),
            Self::SettleHlpFlash => return Ok(Vec::new()),
            Self::CloseInsolventHlp(value) => borsh::to_vec(value),

            Self::QueueParameterProposal | Self::ExecuteParameterProposal => return Ok(Vec::new()),
            Self::DelegatedCloseLeverage(value) => borsh::to_vec(value),
            Self::BeforeTakeProfit(value)
            | Self::AfterCloseOrder(value)
            | Self::BeforeStopLoss(value)
            | Self::ExecuteHlpOrder(value) => borsh::to_vec(value),
            Self::ExecuteEntryOrder(value) => borsh::to_vec(value),
            Self::SettleProtocolAuction(value) => borsh::to_vec(value),
        };
        result.map_err(|error| {
            InstructionEncodingError::new(
                InstructionEncodingErrorCode::InvalidArguments,
                format!("Borsh serialization failed: {error}"),
            )
        })
    }
}

pub fn encode_keeper_instruction(
    contract: &InstructionContract,
    arguments: &KeeperInstructionArguments,
) -> Result<Vec<u8>, InstructionEncodingError> {
    let specification_key = arguments.specification_key();
    let specification = contract
        .instructions
        .iter()
        .find(|entry| entry.key == specification_key)
        .ok_or_else(|| {
            InstructionEncodingError::new(
                InstructionEncodingErrorCode::ContractMismatch,
                format!("{specification_key}: instruction is absent from the pinned contract"),
            )
        })?;
    let (expected_program, expected_name) = specification_key.split_once(':').ok_or_else(|| {
        InstructionEncodingError::new(
            InstructionEncodingErrorCode::ContractMismatch,
            format!("{specification_key}: malformed specification key"),
        )
    })?;
    let actual_program = match specification.program {
        crate::ProgramName::Dusk => "dusk",
        crate::ProgramName::LeverageDelegate => "leverage_delegate",
    };
    if actual_program != expected_program || specification.instruction_name != expected_name {
        return Err(InstructionEncodingError::new(
            InstructionEncodingErrorCode::ContractMismatch,
            format!("{specification_key}: contract instruction identity differs"),
        ));
    }
    let discriminator = exact_hex::<8>(&specification.discriminator_hex).map_err(|message| {
        InstructionEncodingError::new(
            InstructionEncodingErrorCode::ContractMismatch,
            format!("{specification_key}: {message}"),
        )
    })?;
    let anchor_discriminator = Sha256::digest(format!("global:{}", specification.instruction_name));
    if discriminator != anchor_discriminator[..8] {
        return Err(InstructionEncodingError::new(
            InstructionEncodingErrorCode::ContractMismatch,
            format!("{specification_key}: discriminator differs from Anchor global hash"),
        ));
    }
    let mut output = discriminator.to_vec();
    output.extend(arguments.borsh_bytes()?);
    Ok(output)
}

/// Parses the lossless JSON wire form and returns native typed arguments.
pub fn parse_keeper_instruction_arguments(
    specification_key: &str,
    value: Value,
) -> Result<KeeperInstructionArguments, InstructionEncodingError> {
    match specification_key {
        "dusk:observe_liquidation" => {
            let wire: ObserveLiquidationWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::ObserveLiquidation(
                ObserveLiquidationArgs {
                    debt_asset: wire.debt_asset,
                },
            ))
        }
        "dusk:preview_flash_liquidation" => {
            let wire: PreviewFlashLiquidationWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::PreviewFlashLiquidation(
                PreviewFlashLiquidationArgs {
                    debt_asset: wire.debt_asset,
                    max_repayment: decimal_u64(&wire.max_repayment)?,
                    full: wire.full,
                },
            ))
        }
        "dusk:preview_emergency_liquidation" => {
            let wire: PreviewEmergencyLiquidationWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::PreviewEmergencyLiquidation(
                PreviewEmergencyLiquidationArgs {
                    debt_asset: wire.debt_asset,
                    collateral_debit: decimal_u64(&wire.collateral_debit)?,
                    full: wire.full,
                },
            ))
        }
        "dusk:begin_flash_liquidation" => {
            let wire: BeginFlashLiquidationWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::BeginFlashLiquidation(
                BeginFlashLiquidationArgs {
                    position: bs58::decode(&wire.position)
                        .into_vec()
                        .ok()
                        .and_then(|v| v.try_into().ok())
                        .ok_or_else(|| {
                            InstructionEncodingError::new(
                                InstructionEncodingErrorCode::InvalidArguments,
                                "position must be a pubkey",
                            )
                        })?,
                    debt_asset: wire.debt_asset,
                    max_repayment: decimal_u64(&wire.max_repayment)?,
                    max_payment: decimal_u64(&wire.max_payment)?,
                    min_collateral_credit: decimal_u64(&wire.min_collateral_credit)?,
                    min_buyer_credit: decimal_u64(&wire.min_buyer_credit)?,
                    full: wire.full,
                    settle_index: wire.settle_index,
                },
            ))
        }
        "dusk:emergency_liquidation" => {
            let wire: EmergencyLiquidationWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::EmergencyLiquidation(
                EmergencyLiquidationArgs {
                    debt_asset: wire.debt_asset,
                    collateral_debit: decimal_u64(&wire.collateral_debit)?,
                    full: wire.full,
                    min_reward_credit: decimal_u64(&wire.min_reward_credit)?,
                    min_swap_output: decimal_u64(&wire.min_swap_output)?,
                },
            ))
        }
        "dusk:settle_flash_liquidation" => {
            parse_empty(value)?;
            Ok(KeeperInstructionArguments::SettleFlashLiquidation)
        }
        "dusk:preview_hlp_flash" => {
            let wire: PreviewHlpFlashWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::PreviewHlpFlash(
                PreviewHlpFlashArgs {
                    target_asset: wire.target_asset,
                },
            ))
        }
        "dusk:begin_hlp_flash" => {
            let wire: BeginHlpFlashWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::BeginHlpFlash(
                BeginHlpFlashArgs {
                    target_asset: wire.target_asset,
                    max_payment: decimal_u64(&wire.max_payment)?,
                    allow_full: wire.allow_full,
                    max_insurance_draw: decimal_u64(&wire.max_insurance_draw)?,
                    max_socialized_loss: decimal_u64(&wire.max_socialized_loss)?,
                    max_collateral_debit: decimal_u64(&wire.max_collateral_debit)?,
                    min_collateral_credit: decimal_u64(&wire.min_collateral_credit)?,
                    settle_index: wire.settle_index,
                },
            ))
        }
        "dusk:settle_hlp_flash" => {
            parse_empty(value)?;
            Ok(KeeperInstructionArguments::SettleHlpFlash)
        }
        "dusk:close_insolvent_hlp" => {
            let wire: CloseInsolventHlpWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::CloseInsolventHlp(
                CloseInsolventHlpArgs {
                    target_asset: wire.target_asset,
                    max_insurance_draw: decimal_u64(&wire.max_insurance_draw)?,
                    max_socialized_loss: decimal_u64(&wire.max_socialized_loss)?,
                    min_caller_bounty_out: decimal_u64(&wire.min_caller_bounty_out)?,
                },
            ))
        }

        "dusk:delegated_close_leverage" => {
            let wire: DelegatedCloseWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::DelegatedCloseLeverage(
                DelegatedCloseLeverageArgs {
                    debt_asset: wire.debt_asset,
                    min_amount_out: decimal_u64(&wire.min_amount_out)?,
                    close_bps: wire.close_bps,
                    delegated: DelegatedCpiArgs {
                        before_ix_data: variable_hex(&wire.before_ix_data_hex)?,
                        after_ix_data: variable_hex(&wire.after_ix_data_hex)?,
                        before_accounts_len: wire.before_accounts_len,
                    },
                },
            ))
        }
        "leverage_delegate:before_take_profit" => {
            parse_order(value).map(KeeperInstructionArguments::BeforeTakeProfit)
        }
        "leverage_delegate:after_close_order" => {
            parse_order(value).map(KeeperInstructionArguments::AfterCloseOrder)
        }
        "leverage_delegate:before_stop_loss" => {
            parse_order(value).map(KeeperInstructionArguments::BeforeStopLoss)
        }
        "leverage_delegate:execute_hlp_order" => {
            parse_order(value).map(KeeperInstructionArguments::ExecuteHlpOrder)
        }
        "leverage_delegate:execute_leverage_entry_order" => {
            let wire: ExecuteEntryOrderWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::ExecuteEntryOrder(
                ExecuteEntryOrderArgs {
                    order_id: decimal_u64(&wire.order_id)?,
                    min_owner_refund_out: decimal_u64(&wire.min_owner_refund_out)?,
                },
            ))
        }
        "dusk:settle_protocol_auction" => {
            let wire: SettleProtocolWire = parse_wire(value)?;
            Ok(KeeperInstructionArguments::SettleProtocolAuction(
                SettleProtocolAuctionArgs {
                    lane: wire.lane.into(),
                    source: wire.source.into(),
                    sold_amount: decimal_u64(&wire.sold_amount)?,
                    max_payment_amount: decimal_u64(&wire.max_payment_amount)?,
                },
            ))
        }
        "dusk:queue_parameter_proposal" => {
            parse_empty(value)?;
            Ok(KeeperInstructionArguments::QueueParameterProposal)
        }
        "dusk:execute_parameter_proposal" => {
            parse_empty(value)?;
            Ok(KeeperInstructionArguments::ExecuteParameterProposal)
        }
        _ => Err(InstructionEncodingError::new(
            InstructionEncodingErrorCode::UnsupportedInstruction,
            format!("{specification_key}: unsupported keeper instruction"),
        )),
    }
}

pub fn encode_keeper_instruction_json(
    contract: &InstructionContract,
    specification_key: &str,
    value: Value,
) -> Result<Vec<u8>, InstructionEncodingError> {
    let arguments = parse_keeper_instruction_arguments(specification_key, value)?;
    encode_keeper_instruction(contract, &arguments)
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProtocolAuctionLaneWire {
    Fee,
    Buyback,
}

impl From<ProtocolAuctionLaneWire> for ProtocolAuctionLane {
    fn from(value: ProtocolAuctionLaneWire) -> Self {
        match value {
            ProtocolAuctionLaneWire::Fee => Self::Fee,
            ProtocolAuctionLaneWire::Buyback => Self::Buyback,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProtocolRevenueSourceWire {
    Swap,
    Interest,
}

impl From<ProtocolRevenueSourceWire> for ProtocolRevenueSource {
    fn from(value: ProtocolRevenueSourceWire) -> Self {
        match value {
            ProtocolRevenueSourceWire::Swap => Self::Swap,
            ProtocolRevenueSourceWire::Interest => Self::Interest,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DelegatedCloseWire {
    debt_asset: u8,
    min_amount_out: String,
    close_bps: u16,
    before_ix_data_hex: String,
    after_ix_data_hex: String,
    before_accounts_len: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrderWire {
    order_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettleProtocolWire {
    lane: ProtocolAuctionLaneWire,
    source: ProtocolRevenueSourceWire,
    sold_amount: String,
    max_payment_amount: String,
}

fn parse_empty(value: Value) -> Result<(), InstructionEncodingError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EmptyArguments {}
    parse_wire::<EmptyArguments>(value).map(|_| ())
}

fn parse_order(value: Value) -> Result<ExecuteOrderArgs, InstructionEncodingError> {
    let wire: OrderWire = parse_wire(value)?;
    Ok(ExecuteOrderArgs {
        order_id: decimal_u64(&wire.order_id)?,
    })
}

fn parse_wire<T: DeserializeOwned>(value: Value) -> Result<T, InstructionEncodingError> {
    serde_json::from_value(value).map_err(|error| {
        InstructionEncodingError::new(
            InstructionEncodingErrorCode::InvalidArguments,
            format!("invalid instruction arguments: {error}"),
        )
    })
}

fn decimal_u64(value: &str) -> Result<u64, InstructionEncodingError> {
    if value.is_empty()
        || (value != "0" && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_argument(
            "u64 values must use canonical decimal strings",
        ));
    }
    value
        .parse::<u64>()
        .map_err(|_| invalid_argument("u64 value is outside 0..=18446744073709551615"))
}

fn variable_hex(value: &str) -> Result<Vec<u8>, InstructionEncodingError> {
    if !value.len().is_multiple_of(2) {
        return Err(invalid_argument(
            "byte vectors must use even-length lowercase hex",
        ));
    }
    decode_hex_bytes(value).map_err(invalid_argument)
}

fn exact_hex<const N: usize>(value: &str) -> Result<[u8; N], &'static str> {
    let bytes = decode_hex_bytes(value)?;
    bytes
        .try_into()
        .map_err(|_| "discriminator does not contain exactly eight bytes")
}

fn decode_hex_bytes(value: &str) -> Result<Vec<u8>, &'static str> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("value is not even-length lowercase hex");
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0]).ok_or("value is not lowercase hex")?;
            let low = hex_nibble(pair[1]).ok_or("value is not lowercase hex")?;
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

fn invalid_argument(message: impl Into<String>) -> InstructionEncodingError {
    InstructionEncodingError::new(InstructionEncodingErrorCode::InvalidArguments, message)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionEncodingErrorCode {
    UnsupportedInstruction,
    InvalidArguments,
    ContractMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstructionEncodingError {
    pub code: InstructionEncodingErrorCode,
    message: String,
}

impl InstructionEncodingError {
    fn new(code: InstructionEncodingErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for InstructionEncodingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for InstructionEncodingError {}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTRACT: &str = include_str!("../../../../protocol/keeper-instructions.v1.json");
    const FIXTURES: &str =
        include_str!("../../../../fixtures/conformance/v1/adapter-codec-cases.json");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureBundle {
        encoding_cases: Vec<EncodingCase>,
        invalid_encoding_cases: Vec<InvalidEncodingCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct EncodingCase {
        name: String,
        specification_key: String,
        arguments: Value,
        expected_data_hex: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct InvalidEncodingCase {
        name: String,
        specification_key: String,
        arguments: Value,
        expected_error: InstructionEncodingErrorCode,
    }

    fn contract() -> InstructionContract {
        InstructionContract::from_json(CONTRACT).expect("instruction contract must parse")
    }

    #[test]
    fn native_borsh_encoding_matches_every_shared_golden_vector() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("adapter codec fixtures must parse");
        assert_eq!(fixtures.encoding_cases.len(), contract().instructions.len());
        for fixture in fixtures.encoding_cases {
            let encoded = encode_keeper_instruction_json(
                &contract(),
                &fixture.specification_key,
                fixture.arguments,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
            assert_eq!(hex(&encoded), fixture.expected_data_hex, "{}", fixture.name);
        }
    }

    #[test]
    fn rejects_every_shared_invalid_argument_vector() {
        let fixtures: FixtureBundle =
            serde_json::from_str(FIXTURES).expect("adapter codec fixtures must parse");
        for fixture in fixtures.invalid_encoding_cases {
            let error = encode_keeper_instruction_json(
                &contract(),
                &fixture.specification_key,
                fixture.arguments,
            )
            .unwrap_err();
            assert_eq!(error.code, fixture.expected_error, "{}", fixture.name);
        }
    }

    #[test]
    fn rejects_a_discriminator_drift_in_the_contract() {
        let mut contract = contract();
        contract
            .instructions
            .iter_mut()
            .find(|s| s.key == "dusk:settle_flash_liquidation")
            .unwrap()
            .discriminator_hex = "00".repeat(8);
        let error = encode_keeper_instruction(
            &contract,
            &KeeperInstructionArguments::SettleFlashLiquidation,
        )
        .expect_err("a contract mismatch must fail closed");
        assert_eq!(error.code, InstructionEncodingErrorCode::ContractMismatch);
    }

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(DIGITS[(byte >> 4) as usize] as char);
            output.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
        output
    }
}
