//! Transaction compilation and unsigned RPC simulation shared by workers.
use crate::{
    liquidation::plan::{Snapshot, address, key},
    runtime::rpc::RpcClient,
    runtime::transaction::{Instruction, base64, compile_message, serialize_transaction},
};
type Result<T> = std::result::Result<T, String>;
pub(crate) fn compile_message_for(
    payer: [u8; 32],
    s: &Snapshot,
    ixs: &[Instruction],
    hash: [u8; 32],
) -> Result<Vec<u8>> {
    if let Ok(table) = std::env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
        let table = key(&table)?;
        let a = s.account(table)?;
        if a.owner != "AddressLookupTab1e1111111111111111111111111"
            || a.data.len() < 56
            || (a.data.len() - 56) % 32 != 0
            || a.data[..4] != 1u32.to_le_bytes()
        {
            return Err("invalid liquidation lookup table".into());
        }
        let deactivated = u64::from_le_bytes(a.data[4..12].try_into().unwrap());
        let extended = u64::from_le_bytes(a.data[12..20].try_into().unwrap());
        if deactivated != u64::MAX || extended >= s.slot {
            return Err("lookup table deactivated or not warmed up".into());
        }
        let addresses = a.data[56..]
            .chunks_exact(32)
            .map(|v| v.try_into().unwrap())
            .collect::<Vec<_>>();
        return crate::runtime::transaction::compile_v0_message(
            payer, ixs, hash, table, &addresses,
        )
        .map_err(|e| e.to_string());
    }
    compile_message(payer, ixs, hash).map_err(|e| e.to_string())
}
pub(crate) fn simulate_for(
    client: &RpcClient,
    payer: [u8; 32],
    snapshot: &Snapshot,
    ixs: &[Instruction],
    post: &[[u8; 32]],
) -> Result<serde_json::Value> {
    let message = compile_message_for(payer, snapshot, ixs, [0; 32])?;
    let wire = serialize_transaction(&[[0; 64]], &message);
    if wire.len() > 1232 {
        return Err(format!(
            "transaction is {} bytes; configure a liquidation lookup table",
            wire.len()
        ));
    }
    client.call("simulateTransaction",serde_json::json!([base64(&wire),{"encoding":"base64","commitment":"confirmed","minContextSlot":snapshot.slot,"sigVerify":false,"replaceRecentBlockhash":true,"accounts":{"encoding":"base64","addresses":post.iter().copied().map(address).collect::<Vec<_>>()}}])).map_err(|e|e.to_string())
}
