//! Explicit local-devnet testing exception. Other networks retain the frozen gate.
use super::rpc::{RpcClient, decode_base64};
use dusk_adapter::{LockStatus, ProtocolLock};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path};

const GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
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
    binary_sha256: String,
    idl_account: String,
    idl_account_sha256: String,
}

pub fn enabled() -> bool {
    env::var("KEEPER_ALLOW_CAPTURED_DEVNET").as_deref() == Ok("1")
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn account_bytes(account: &serde_json::Value, owner: &str, executable: bool) -> Result<Vec<u8>> {
    if account["owner"] != owner || account["executable"] != executable {
        return Err("devnet deployment account identity changed".into());
    }
    if account["data"][1] != "base64" {
        return Err("devnet deployment account encoding".into());
    }
    decode_base64(
        account["data"][0]
            .as_str()
            .ok_or("missing deployment account")?,
    )
    .ok_or("invalid deployment account bytes".into())
}

fn check_program(program: &Program, accounts: &[serde_json::Value], full: bool) -> Result<()> {
    if accounts.len() != 2 {
        return Err("incomplete devnet deployment snapshot".into());
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
        return Err(format!(
            "{} devnet deployment changed; recapture required",
            program.name
        ));
    }
    if full && hash(&data[45..]) != program.binary_sha256 {
        return Err(format!("{} deployed executable hash changed", program.name));
    }
    Ok(())
}

/// Full verification at startup; upgrade metadata is rechecked each pass and send.
pub fn verify(client: &RpcClient, full: bool) -> Result<()> {
    let lock_path = env::var("DUSK_PROTOCOL_LOCK").unwrap_or_else(|_| "protocol.lock.json".into());
    let lock = ProtocolLock::read(&lock_path).map_err(|e| e.to_string())?;
    if !matches!(lock.status, LockStatus::Captured | LockStatus::Frozen) {
        return Err("devnet execution requires captured or frozen artifacts".into());
    }
    let root = Path::new(&lock_path).parent().unwrap_or(Path::new("."));
    let deployment: Deployment = serde_json::from_slice(
        &fs::read(root.join("protocol/devnet-deployment.v1.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if deployment.cluster != "devnet"
        || deployment.genesis_hash != GENESIS
        || deployment.protocol_revision != lock.revision
        || deployment.source_commit != lock.source.git_commit
        || deployment.programs.len() != lock.programs.len()
        || deployment.programs.is_empty()
    {
        return Err("devnet capture does not match protocol lock".into());
    }
    let genesis: String = client
        .call("getGenesisHash", serde_json::json!([]))
        .map_err(|e| e.to_string())?;
    if genesis != GENESIS {
        return Err("devnet test exception refuses this network".into());
    }
    if full {
        for artifact in lock
            .programs
            .iter()
            .flat_map(|p| [&p.binary, &p.idl])
            .chain([&lock.sdk])
        {
            let expected = artifact.sha256.as_ref().ok_or("unpinned devnet artifact")?;
            if hash(&fs::read(root.join(&artifact.path)).map_err(|e| e.to_string())?) != *expected {
                return Err("local devnet artifact hash mismatch".into());
            }
        }
    }
    for observed in &deployment.programs {
        let pinned = lock
            .programs
            .iter()
            .find(|p| p.name == observed.name)
            .ok_or("unexpected devnet program")?;
        if pinned.program_id.as_ref() != Some(&observed.program_id)
            || pinned.binary.sha256.as_ref() != Some(&observed.binary_sha256)
        {
            return Err("devnet program pin differs from capture".into());
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
            .map_err(|e| e.to_string())?;
        check_program(
            observed,
            response["value"]
                .as_array()
                .ok_or("missing devnet program accounts")?,
            full,
        )?;
        if full {
            let response: serde_json::Value = client.call("getAccountInfo", serde_json::json!([observed.idl_account,{"encoding":"base64","commitment":"confirmed","minContextSlot":response["context"]["slot"]}])).map_err(|e| e.to_string())?;
            let bytes = account_bytes(&response["value"], &observed.program_id, false)?;
            if hash(&bytes) != observed.idl_account_sha256 {
                return Err(format!("{} deployed IDL changed", observed.name));
            }
        }
    }
    Ok(())
}

/// Idempotent wallet account preparation through the same durable submit path.
pub fn prepare_wallet(
    client: &RpcClient,
    context: &crate::workers::WorkerContext<'_>,
    minimum_lamports: u64,
) -> Result<Option<String>> {
    use super::{
        accounts::{ASSOCIATED_TOKEN_PROGRAM, MarketAccounts, associated_token_account},
        flash_journal::{Journal, recover_pending},
        transaction::{AccountMeta, Instruction},
    };
    use crate::liquidation::{
        plan::{Snapshot, address, key},
        simulation, state_hash,
    };
    if !enabled() || context.dry_run {
        return Err("wallet preparation requires explicit live devnet testing".into());
    }
    // This target applies only to the one-off wallet preparation command.
    // Worker discovery and execution cover every supported market.
    let market = key(&env::var("KEEPER_PREPARE_MARKET")
        .map_err(|_| "wallet preparation requires KEEPER_PREPARE_MARKET")?)?;
    recover_pending(client, &context.contract.protocol_revision, false)
        .map_err(|e| e.to_string())?;
    let first = Snapshot::fetch(client, &[market], 0).map_err(|e| e.to_string())?;
    let decoded = MarketAccounts::decode(
        context.layout,
        market,
        first.owned(market, context.program_id)?,
    )
    .ok_or("invalid devnet market")?;
    let token = key(dusk_adapter::transfer_fees::TOKEN_PROGRAM)?;
    let buyer = context.signer.public_key();
    let mints = [decoded.base_mint, decoded.quote_mint];
    let atas = mints.map(|mint| associated_token_account(buyer, token, mint).unwrap());
    let mut keys = vec![market, buyer];
    keys.extend(mints);
    keys.extend(atas);
    let snapshot = Snapshot::fetch(client, &keys, first.slot).map_err(|e| e.to_string())?;
    let mut instructions = vec![];
    for (mint, ata) in mints.into_iter().zip(atas) {
        snapshot.owned(mint, token)?;
        if let Some(Some(account)) = snapshot.accounts.get(&address(ata)) {
            if account.owner != address(token)
                || account.data.get(..32) != Some(mint.as_slice())
                || account.data.get(32..64) != Some(buyer.as_slice())
                || account.data.get(108) != Some(&1)
            {
                return Err("invalid wallet token account".into());
            }
            continue;
        }
        instructions.push(Instruction {
            program_id: key(ASSOCIATED_TOKEN_PROGRAM)?,
            data: vec![1],
            accounts: vec![
                AccountMeta::signer(buyer),
                AccountMeta::writable(ata),
                AccountMeta::readonly(buyer),
                AccountMeta::readonly(mint),
                AccountMeta::readonly([0; 32]),
                AccountMeta::readonly(token),
            ],
        });
    }
    if instructions.is_empty() {
        return Ok(None);
    }
    let rent: u64 = client
        .call(
            "getMinimumBalanceForRentExemption",
            serde_json::json!([165]),
        )
        .map_err(|e| e.to_string())?;
    let balance = client
        .lamports(&address(buyer))
        .map_err(|e| e.to_string())?;
    let cost = rent
        .checked_mul(instructions.len() as u64)
        .and_then(|v| v.checked_add(10_000))
        .and_then(|v| v.checked_add(minimum_lamports))
        .ok_or("wallet preparation cost overflow")?;
    if balance < cost {
        return Err("wallet preparation would consume the configured SOL reserve".into());
    }
    let result = simulation::simulate_for(client, buyer, &snapshot, &instructions, &atas)?;
    if !result["value"]["err"].is_null() {
        return Err(format!(
            "wallet preparation simulation: {}",
            result["value"]["err"]
        ));
    }
    let fresh = Snapshot::fetch(client, &keys, snapshot.slot).map_err(|e| e.to_string())?;
    let fingerprint = state_hash(&fresh, 0)?;
    if fingerprint != state_hash(&snapshot, 0)? {
        return Err("wallet preparation state changed".into());
    }
    verify(client, false)?;
    let block: serde_json::Value = client
        .call(
            "getLatestBlockhash",
            serde_json::json!([{"commitment":"confirmed","minContextSlot":fresh.slot}]),
        )
        .map_err(|e| e.to_string())?;
    let hash = block["value"]["blockhash"]
        .as_str()
        .ok_or("blockhash missing")?;
    let height = block["value"]["lastValidBlockHeight"]
        .as_u64()
        .ok_or("block height missing")?;
    let message = simulation::compile_message_for(buyer, &fresh, &instructions, key(hash)?)?;
    let mut journal = Journal::connect(
        &env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required before signing")?,
    )?;
    journal.submit(
        client,
        context.signer,
        &context.contract.protocol_revision,
        &format!("wallet:{}", address(buyer)),
        0,
        &address(market),
        "prepare_devnet_wallet",
        fresh.slot,
        &fingerprint,
        &message,
        hash,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::transaction::base64;

    #[test]
    fn deployment_recheck_rejects_upgrade_pointer_slot_bytes_and_owner_changes() {
        let program_data = [7u8; 32];
        let mut executable = 2u32.to_le_bytes().to_vec();
        executable.extend(program_data);
        let mut data = vec![0; 45];
        data[..4].copy_from_slice(&3u32.to_le_bytes());
        data[4..12].copy_from_slice(&123u64.to_le_bytes());
        data.extend(b"executable");
        let program = Program {
            name: "dusk".into(),
            program_id: "unused".into(),
            program_data: bs58::encode(program_data).into_string(),
            last_upgrade_slot: 123,
            binary_sha256: hash(&data[45..]),
            idl_account: "unused".into(),
            idl_account_sha256: "unused".into(),
        };
        let accounts = vec![
            serde_json::json!({"owner":LOADER,"executable":true,"data":[base64(&executable),"base64"]}),
            serde_json::json!({"owner":LOADER,"executable":false,"data":[base64(&data),"base64"]}),
        ];
        assert!(check_program(&program, &accounts, true).is_ok());
        for index in [4, 45] {
            let mut changed = data.clone();
            changed[index] ^= 1;
            let mut broken = accounts.clone();
            broken[1]["data"][0] = base64(&changed).into();
            assert!(check_program(&program, &broken, true).is_err());
        }
        let mut broken = accounts.clone();
        broken[1]["owner"] = "wrong".into();
        assert!(check_program(&program, &broken, false).is_err());
        let mut pointer = executable.clone();
        pointer[4] ^= 1;
        let mut broken = accounts.clone();
        broken[0]["data"][0] = base64(&pointer).into();
        assert!(check_program(&program, &broken, false).is_err());
    }
}
