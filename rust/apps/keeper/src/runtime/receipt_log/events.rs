//! Pinned Dusk event prefixes. The remaining position/snapshot fields are not
//! needed for presentation; native decoding is checked against Anchor fixtures.
use super::wire::Instruction;
use borsh::BorshDeserialize;
use dusk_adapter::liquidation::{
    EmergencyLiquidationQuote, LiquidationFeeAllocation, LiquidationLossAllocation,
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

pub(super) struct Protocol {
    pub program: String,
    instructions: BTreeMap<Vec<u8>, (String, Vec<String>)>,
    events: BTreeMap<Vec<u8>, String>,
}
impl Protocol {
    pub fn load() -> Result<&'static Self, &'static String> {
        static IDL: OnceLock<Result<Protocol, String>> = OnceLock::new();
        IDL.get_or_init(|| {
            let raw = std::fs::read_to_string("artifacts/dusk.json").map_err(|e| e.to_string())?;
            let idl: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            let mut instructions = BTreeMap::new();
            for ix in idl["instructions"]
                .as_array()
                .ok_or("IDL instructions unavailable")?
            {
                let mut accounts = Vec::new();
                flatten(&ix["accounts"], &mut accounts)?;
                instructions.insert(
                    discriminator(&ix["discriminator"])?,
                    (
                        ix["name"]
                            .as_str()
                            .ok_or("IDL instruction name")?
                            .to_owned(),
                        accounts,
                    ),
                );
            }
            let mut events = BTreeMap::new();
            for e in idl["events"].as_array().ok_or("IDL events unavailable")? {
                events.insert(
                    discriminator(&e["discriminator"])?,
                    e["name"].as_str().ok_or("IDL event name")?.to_owned(),
                );
            }
            Ok(Self {
                program: idl["address"]
                    .as_str()
                    .ok_or("IDL address unavailable")?
                    .to_owned(),
                instructions,
                events,
            })
        })
        .as_ref()
    }

    pub fn accounts<'a>(&'a self, ix: &Instruction) -> Option<(&'a str, &'a [String])> {
        if ix.program != self.program {
            return None;
        }
        self.instructions
            .get(ix.data.get(..8)?)
            .map(|(name, accounts)| (name.as_str(), accounts.as_slice()))
    }

    pub fn event(&self, ix: &Instruction) -> Option<Result<Event, String>> {
        // Anchor emit_cpi! dispatch tag, followed by the event discriminator.
        if ix.program != self.program
            || ix.data.get(..8) != Some(&[228, 69, 165, 46, 81, 203, 154, 29])
        {
            return None;
        }
        let name = self.events.get(ix.data.get(8..16)?)?;
        let mut bytes = ix.data.get(16..)?;
        Some(
            match name.as_str() {
                "SwapExecuted" => Swap::deserialize(&mut bytes).map(Event::Swap),
                "EmergencyLiquidationSettled" => {
                    Emergency::deserialize(&mut bytes).map(|e| Event::Emergency(Box::new(e)))
                }
                "FlashLiquidationSettled" => Flash::deserialize(&mut bytes).map(Event::Flash),
                _ => return None,
            }
            .map_err(|e| format!("{name} could not be decoded: {e}")),
        )
    }
}

fn discriminator(value: &Value) -> Result<Vec<u8>, String> {
    let bytes: Vec<u8> = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if bytes.len() != 8 {
        return Err("IDL discriminator length".into());
    }
    Ok(bytes)
}
fn flatten(value: &Value, out: &mut Vec<String>) -> Result<(), String> {
    for account in value.as_array().ok_or("IDL instruction accounts")? {
        if account["accounts"].is_array() {
            flatten(&account["accounts"], out)?;
        } else {
            out.push(
                account["name"]
                    .as_str()
                    .ok_or("IDL account name")?
                    .to_owned(),
            );
        }
    }
    Ok(())
}

pub(super) enum Event {
    Swap(Swap),
    Emergency(Box<Emergency>),
    Flash(Flash),
}

#[derive(BorshDeserialize)]
pub(super) struct Swap {
    pub market: [u8; 32],
    _trader: [u8; 32],
    _actor: [u8; 32],
    _position: Option<[u8; 32]>,
    _origin: u8,
    _slot: u64,
    pub asset_in_side: u8,
    pub amount_in: u64,
    pub amount_out: u64,
    pub gross_amount_out: u64,
    pub fee_asset_side: u8,
    pub amount_in_after_fee: u64,
    pub base_fee: u64,
    pub divergence_fee: u64,
    pub volatility_fee: u64,
    pub retained_fee: u64,
    pub compounded_fee: u64,
}

#[derive(BorshDeserialize)]
pub(super) struct Emergency {
    pub market: [u8; 32],
    pub position: [u8; 32],
    pub owner: [u8; 32],
    pub caller: [u8; 32],
    _leverage: bool,
    pub debt_asset: u8,
    pub quote: EmergencyLiquidationQuote,
    pub remaining_collateral: u64,
    pub remaining_debt: u64,
}

#[derive(BorshDeserialize)]
pub(super) struct Flash {
    pub market: [u8; 32],
    pub position: [u8; 32],
    pub buyer: [u8; 32],
    _leverage: bool,
    pub debt_asset: u8,
    pub collateral_debit: u64,
    pub debt_repaid: u64,
    pub owner_surplus: u64,
    pub fee: LiquidationFeeAllocation,
    pub loss: LiquidationLossAllocation,
    pub remaining_collateral: u64,
    pub remaining_debt: u64,
}
