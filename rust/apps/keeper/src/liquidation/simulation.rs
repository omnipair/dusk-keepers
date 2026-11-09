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
    let mut tables = Vec::new();
    // Route snapshots contain only explicitly requested tables, fetched from
    // RPC. Never compile against addresses supplied by an external quote API.
    let mut keys = s
        .accounts
        .iter()
        .filter_map(|(k, a)| {
            a.as_ref()
                .filter(|a| a.owner == "AddressLookupTab1e1111111111111111111111111")
                .map(|_| key(k))
        })
        .collect::<Result<Vec<_>>>()?;
    if let Ok(table) = std::env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
        keys.insert(0, key(&table)?);
    }
    let mut seen = std::collections::BTreeSet::new();
    for table in keys {
        if !seen.insert(table) {
            continue;
        }
        let a = s.account(table)?;
        tables.push((table, lookup_addresses(a, s.slot)?));
    }
    if !tables.is_empty() {
        return crate::runtime::transaction::compile_v0_message_with_tables(
            payer, ixs, hash, &tables,
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

fn lookup_addresses(a: &super::plan::Account, slot: u64) -> Result<Vec<[u8; 32]>> {
    if a.owner != "AddressLookupTab1e1111111111111111111111111"
        || a.executable
        || a.data.len() < 56
        || !(a.data.len() - 56).is_multiple_of(32)
        || a.data.len() > 56 + 256 * 32
        || a.data[..4] != 1u32.to_le_bytes()
    {
        return Err("invalid liquidation lookup table".into());
    }
    let deactivated = u64::from_le_bytes(a.data[4..12].try_into().unwrap());
    let extended = u64::from_le_bytes(a.data[12..20].try_into().unwrap());
    if deactivated != u64::MAX || extended >= slot {
        return Err("lookup table deactivated or not warmed up".into());
    }
    Ok(a.data[56..]
        .chunks_exact(32)
        .map(|v| v.try_into().unwrap())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup_tables_require_real_owner_active_state_and_completed_warmup() {
        let mut a = super::super::plan::Account {
            owner: "AddressLookupTab1e1111111111111111111111111".into(),
            data: vec![0; 88],
            executable: false,
            lamports: None,
        };
        a.data[..4].copy_from_slice(&1u32.to_le_bytes());
        a.data[4..12].copy_from_slice(&u64::MAX.to_le_bytes());
        a.data[12..20].copy_from_slice(&99u64.to_le_bytes());
        a.data[56..].fill(7);
        assert_eq!(lookup_addresses(&a, 100).unwrap(), vec![[7; 32]]);
        assert!(lookup_addresses(&a, 99).is_err());
        let valid = a.clone();
        a.data[4..12].fill(0);
        assert!(lookup_addresses(&a, 100).is_err());
        a = valid.clone();
        a.owner = "wrong".into();
        assert!(lookup_addresses(&a, 100).is_err());
        a = valid.clone();
        a.data.pop();
        assert!(lookup_addresses(&a, 100).is_err());
        a = valid;
        a.executable = true;
        assert!(lookup_addresses(&a, 100).is_err());
    }
}
