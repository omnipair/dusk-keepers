//! Offline planner used by SDK parity and bank-execution tests. Never signs.
use crate::flash::{Context, Snapshot, key};
use borsh::BorshDeserialize;
use dusk_adapter::{
    AccountLayoutManifest, InstructionContract,
    liquidation::{EmergencyLiquidationQuote, FlashLiquidationQuote},
};
use std::io::Read;
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    let s: Snapshot = serde_json::from_value(v["snapshot"].clone())?;
    let layout = AccountLayoutManifest::from_json(&std::fs::read_to_string(
        "protocol/keeper-account-layout.v1.json",
    )?)?;
    let contract = InstructionContract::from_json(&std::fs::read_to_string(
        "protocol/keeper-instructions.v1.json",
    )?)?;
    let c = Context::resolve(
        &layout,
        &s,
        key(v["position"].as_str().ok_or("position")?)?,
        key(v["buyer"].as_str().ok_or("buyer")?)?,
        v["kind"].as_str().ok_or("kind")?,
        v["debtAsset"].as_u64().ok_or("side")?.try_into()?,
    )?;
    let quote = crate::discovery::decode_base64(v["quoteBase64"].as_str().unwrap_or(""))
        .ok_or("quote base64")?;
    let max = v["maxRepayment"].as_str().unwrap_or("0").parse()?;
    let ixs = match v["action"].as_str() {
        Some("observe") => vec![c.observation(&contract)?],
        Some("setup") => c.setup(&s)?,
        Some("preview_flash") => {
            vec![c.preview_flash(&contract, max, v["full"].as_bool().unwrap_or(false))?]
        }
        Some("preview_emergency") => vec![c.preview_emergency(
            &contract,
            v["collateralDebit"].as_str().unwrap_or("0").parse()?,
            v["full"].as_bool().unwrap_or(false),
        )?],
        Some("flash") => c.flash(
            &contract,
            &FlashLiquidationQuote::try_from_slice(&quote)?,
            max,
            crate::flash::resources()?,
            serde_json::from_value(v["route"].clone()).unwrap_or_default(),
            v["internal"].as_bool().unwrap_or(true),
        )?,
        Some("emergency") => {
            let mut i = crate::flash::resources()?;
            i.push(c.emergency(
                &contract,
                &EmergencyLiquidationQuote::try_from_slice(&quote)?,
            )?);
            i
        }
        _ => return Err("unknown planner action".into()),
    };
    let legacy = crate::transaction::compile_message(c.buyer, &ixs, [0; 32])?;
    let message = if let Some(table) = v["lookupTable"].as_str() {
        let addresses = v["lookupAddresses"]
            .as_array()
            .ok_or("lookup addresses")?
            .iter()
            .map(|a| key(a.as_str().unwrap_or("")))
            .collect::<Result<Vec<_>, _>>()?;
        crate::transaction::compile_v0_message(c.buyer, &ixs, [0; 32], key(table)?, &addresses)?
    } else {
        legacy.clone()
    };
    println!(
        "{}",
        serde_json::json!({"instructions":ixs,"named":c.named,"messageBase64":crate::transaction::base64(&message),"legacyBytes":crate::transaction::serialize_transaction(&[[0;64]],&legacy).len()})
    );
    Ok(())
}
