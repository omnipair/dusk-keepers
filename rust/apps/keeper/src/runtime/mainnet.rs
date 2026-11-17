//! Frozen mainnet release identity, checked at startup and around live sends.
use super::rpc::{RpcClient, decode_base64};
use dusk_adapter::{LockStatus, ProtocolLock};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, env, fs, path::Path};

pub(crate) const GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
const LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";
type Result<T> = std::result::Result<T, String>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Deployment {
    cluster: String,
    genesis_hash: String,
    protocol_revision: String,
    source_commit: String,
    programs: Vec<Program>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Program {
    name: String,
    program_id: String,
    program_data: String,
    last_upgrade_slot: u64,
    upgrade_authority: Option<String>,
    binary_sha256: String,
    binary_bytes: usize,
    idl_account: String,
    idl_sha256: String,
    idl_account_sha256: String,
}

pub fn enabled() -> bool {
    env::var("KEEPER_MODE").as_deref() == Ok("live") && !super::devnet::enabled()
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn account_bytes(account: &serde_json::Value, owner: &str, executable: bool) -> Result<Vec<u8>> {
    if account["owner"] != owner
        || account["executable"] != executable
        || account["data"][1] != "base64"
    {
        return Err("mainnet deployment account identity or encoding changed".into());
    }
    decode_base64(
        account["data"][0]
            .as_str()
            .ok_or("missing mainnet account data")?,
    )
    .ok_or("invalid mainnet account data".into())
}

fn check_program(program: &Program, accounts: &[serde_json::Value], full: bool) -> Result<()> {
    if accounts.len() != 2 {
        return Err("incomplete mainnet deployment bank".into());
    }
    let executable = account_bytes(&accounts[0], LOADER, true)?;
    let data = account_bytes(&accounts[1], LOADER, false)?;
    if executable.len() != 36
        || executable[..4] != 2u32.to_le_bytes()
        || bs58::encode(&executable[4..36]).into_string() != program.program_data
        || data.len() < 45
        || data[..4] != 3u32.to_le_bytes()
        || u64::from_le_bytes(data[4..12].try_into().unwrap()) != program.last_upgrade_slot
    {
        return Err(format!("{} mainnet deployment changed", program.name));
    }
    let authority = match data[12] {
        0 => None,
        1 => Some(bs58::encode(&data[13..45]).into_string()),
        _ => return Err("invalid mainnet upgrade authority encoding".into()),
    };
    if authority != program.upgrade_authority {
        return Err(format!("{} upgrade authority changed", program.name));
    }
    if full
        && (data.len() != 45 + program.binary_bytes || hash(&data[45..]) != program.binary_sha256)
    {
        return Err(format!("{} deployed executable changed", program.name));
    }
    Ok(())
}

pub fn verify(client: &RpcClient, full: bool) -> Result<()> {
    verify_inner(client, full, true)
}

/// Read-only comparison for a release candidate. This never authorizes signing.
pub fn verify_capture(client: &RpcClient) -> Result<()> {
    verify_inner(client, true, false)
}

fn verify_inner(client: &RpcClient, full: bool, require_frozen: bool) -> Result<()> {
    let path = env::var("DUSK_PROTOCOL_LOCK").unwrap_or_else(|_| "protocol.lock.json".into());
    let lock = ProtocolLock::read(&path).map_err(|_| "mainnet protocol lock unavailable")?;
    if require_frozen && lock.status != LockStatus::Frozen {
        return Err("mainnet requires a frozen protocol lock".into());
    }
    if require_frozen {
        lock.assert_live_ready()
            .map_err(|_| "mainnet protocol lock is incomplete")?;
    } else if !matches!(lock.status, LockStatus::Captured | LockStatus::Frozen) {
        return Err("deployment comparison requires captured artifacts".into());
    }
    let root = Path::new(&path).parent().unwrap_or(Path::new("."));
    let deployment: Deployment = serde_json::from_slice(
        &fs::read(root.join("protocol/mainnet-deployment.v1.json"))
            .map_err(|_| "mainnet deployment capture unavailable")?,
    )
    .map_err(|_| "invalid mainnet deployment capture")?;
    let names: BTreeSet<_> = deployment
        .programs
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    if deployment.cluster != "mainnet-beta"
        || deployment.genesis_hash != GENESIS
        || deployment.protocol_revision != lock.revision
        || deployment.source_commit != lock.source.git_commit
        || deployment.programs.len() != 2
        || lock.programs.len() != 2
        || names != BTreeSet::from(["dusk", "leverage_delegate"])
    {
        return Err("mainnet deployment capture differs from frozen lock".into());
    }
    let genesis: String = client
        .call("getGenesisHash", serde_json::json!([]))
        .map_err(|_| "mainnet genesis RPC unavailable")?;
    if genesis != GENESIS {
        return Err("live release refuses this RPC genesis".into());
    }
    if full {
        for artifact in lock
            .programs
            .iter()
            .flat_map(|p| [&p.binary, &p.idl])
            .chain([&lock.sdk])
        {
            let bytes =
                fs::read(root.join(&artifact.path)).map_err(|_| "mainnet artifact unavailable")?;
            if artifact.sha256.as_deref() != Some(hash(&bytes).as_str()) {
                return Err("mainnet local artifact fingerprint changed".into());
            }
        }
    }
    for observed in &deployment.programs {
        let pin = lock
            .programs
            .iter()
            .find(|p| p.name == observed.name)
            .ok_or("unrecognized mainnet program")?;
        if pin.program_id.as_ref() != Some(&observed.program_id)
            || pin.binary.sha256.as_ref() != Some(&observed.binary_sha256)
            || pin.idl.sha256.as_ref() != Some(&observed.idl_sha256)
        {
            return Err("mainnet capture program/artifact identity mismatch".into());
        }
        let mut options = serde_json::json!({"encoding":"base64","commitment":"confirmed"});
        if !full {
            options["dataSlice"] = serde_json::json!({"offset":0,"length":45});
        }
        let response: serde_json::Value = client
            .call(
                "getMultipleAccounts",
                serde_json::json!([[observed.program_id, observed.program_data], options]),
            )
            .map_err(|_| "mainnet deployment RPC unavailable")?;
        check_program(
            observed,
            response["value"]
                .as_array()
                .ok_or("missing mainnet program accounts")?,
            full,
        )?;
        if full {
            let response: serde_json::Value = client.call("getAccountInfo", serde_json::json!([
                observed.idl_account,{"encoding":"base64","commitment":"confirmed","minContextSlot":response["context"]["slot"]}
            ])).map_err(|_| "mainnet IDL RPC unavailable")?;
            if hash(&account_bytes(
                &response["value"],
                &observed.program_id,
                false,
            )?) != observed.idl_account_sha256
            {
                return Err(format!("{} deployed IDL changed", observed.name));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::transaction::base64;

    #[test]
    fn changed_authority_slot_code_owner_or_programdata_refuses_a_release() {
        let code = vec![1, 2, 3, 4];
        let key = [7u8; 32];
        let data_key = bs58::encode([8u8; 32]).into_string();
        let p = Program {
            name: "dusk".into(),
            program_id: bs58::encode([9u8; 32]).into_string(),
            program_data: data_key,
            last_upgrade_slot: 100,
            upgrade_authority: Some(bs58::encode(key).into_string()),
            binary_sha256: hash(&code),
            binary_bytes: code.len(),
            idl_account: String::new(),
            idl_sha256: String::new(),
            idl_account_sha256: String::new(),
        };
        let mut executable = 2u32.to_le_bytes().to_vec();
        executable.extend([8u8; 32]);
        let mut data = 3u32.to_le_bytes().to_vec();
        data.extend(100u64.to_le_bytes());
        data.push(1);
        data.extend(key);
        data.extend(&code);
        let account = |bytes: &[u8], executable| serde_json::json!({"owner":LOADER,"executable":executable,"data":[base64(bytes),"base64"]});
        let bank = vec![account(&executable, true), account(&data, false)];
        assert!(check_program(&p, &bank, true).is_ok());
        for offset in [4, 13, 45] {
            let mut changed = data.clone();
            changed[offset] ^= 1;
            assert!(check_program(&p, &[bank[0].clone(), account(&changed, false)], true).is_err());
        }
        let mut changed = bank.clone();
        changed[0]["owner"] = serde_json::json!("11111111111111111111111111111111");
        assert!(check_program(&p, &changed, true).is_err());
        executable[4] ^= 1;
        assert!(check_program(&p, &[account(&executable, true), bank[1].clone()], true).is_err());
        let sliced = vec![bank[0].clone(), account(&data[..45], false)];
        assert!(check_program(&p, &sliced, false).is_ok());
    }
}
