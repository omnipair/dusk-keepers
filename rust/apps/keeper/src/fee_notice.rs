//! Poll mints AND Clock: entering the last notice epoch does not write the mint.
use crate::discovery::{DiscoveryError, RpcClient, account_discriminator};
use dusk_adapter::{
    AccountLayoutManifest,
    transfer_fees::{CLOCK, FeePhase, decode_clock_epoch, decode_mint_fees},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeMonitor {
    pub slot: u64,
    pub epoch: u64,
    pub scanned_mints: usize,
    pub final_notice_markets: Vec<String>,
    pub notice_mints: Vec<String>,
    pub final_notice_mints: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeeSnapshot {
    pub slot: u64,
    pub epoch: u64,
    pub fingerprint: Vec<u8>,
    pub phases: Vec<FeePhase>,
}
#[derive(Deserialize)]
struct Context {
    slot: u64,
}
#[derive(Deserialize)]
struct Account {
    owner: String,
    executable: bool,
    data: Vec<String>,
}
#[derive(Deserialize)]
struct Response {
    context: Context,
    value: Vec<Option<Account>>,
}

pub fn read_fees(
    client: &RpcClient,
    mints: &[String],
    min_slot: u64,
) -> Result<FeeSnapshot, DiscoveryError> {
    for attempt in 0..3 {
        match read_fees_once(client, mints, min_slot) {
            Err(DiscoveryError::Malformed(detail))
                if detail == "clock and RPC bank do not match" && attempt < 2 =>
            {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            result => return result,
        }
    }
    unreachable!("fee snapshot retry is bounded")
}

fn read_fees_once(
    client: &RpcClient,
    mints: &[String],
    min_slot: u64,
) -> Result<FeeSnapshot, DiscoveryError> {
    if mints.is_empty() || mints.len() > 99 {
        return Err(DiscoveryError::Malformed("expected 1..99 mints".into()));
    }
    let mut keys = mints.to_vec();
    keys.push(CLOCK.into());
    let response: Response = client.call(
        "getMultipleAccounts",
        serde_json::json!([keys, {
            "encoding":"base64", "commitment":"confirmed", "minContextSlot":min_slot
        }]),
    )?;
    if response.context.slot < min_slot || response.value.len() != keys.len() {
        return Err(DiscoveryError::Malformed(
            "stale or incomplete fee snapshot".into(),
        ));
    }
    let mut accounts = Vec::new();
    for entry in response.value {
        let entry =
            entry.ok_or_else(|| DiscoveryError::Malformed("missing mint or Clock".into()))?;
        if entry.executable || entry.data.len() != 2 || entry.data[1] != "base64" {
            return Err(DiscoveryError::Malformed(
                "invalid fee snapshot account".into(),
            ));
        }
        let data = crate::discovery::decode_base64(&entry.data[0])
            .ok_or_else(|| DiscoveryError::Malformed("invalid fee account encoding".into()))?;
        accounts.push((entry.owner, data));
    }
    let (owner, clock) = accounts.pop().unwrap();
    let epoch = decode_clock_epoch(&owner, &clock, response.context.slot)
        .map_err(DiscoveryError::Malformed)?;
    let mut hash = Sha256::new();
    hash.update(epoch.to_le_bytes());
    let mut phases = Vec::new();
    for (key, (owner, data)) in mints.iter().zip(accounts) {
        let schedule = decode_mint_fees(&owner, &data).map_err(DiscoveryError::Malformed)?;
        phases.push(schedule.map_or(FeePhase::None, |s| s.phase(epoch)));
        // Include authority and schedule writes, including cancellation. More
        // mint changes can conservatively invalidate a quote; never fewer.
        hash.update(key.as_bytes());
        hash.update(owner.as_bytes());
        hash.update(data);
    }
    Ok(FeeSnapshot {
        slot: response.context.slot,
        epoch,
        fingerprint: hash.finalize().to_vec(),
        phases,
    })
}

pub fn monitor(
    client: &RpcClient,
    program: &str,
    layout: &AccountLayoutManifest,
) -> Result<NoticeMonitor, DiscoveryError> {
    let reader = layout
        .reader("Market")
        .map_err(|e| DiscoveryError::Malformed(e.to_string()))?;
    let mut mints = BTreeSet::new();
    let mut market_mints = Vec::new();
    for (market, data) in client.program_accounts(program, account_discriminator("Market"))? {
        for side in ["base_side.asset_mint", "quote_side.asset_mint"] {
            let mint = reader
                .pubkey(side, &data)
                .map_err(|e| DiscoveryError::Malformed(e.to_string()))?;
            let key = bs58::encode(mint).into_string();
            market_mints.push((bs58::encode(market).into_string(), key.clone()));
            mints.insert(key);
        }
    }
    let mints: Vec<_> = mints.into_iter().collect();
    let mut monitor = NoticeMonitor {
        scanned_mints: mints.len(),
        ..NoticeMonitor::default()
    };
    for chunk in mints.chunks(99) {
        let snapshot = read_fees(client, chunk, monitor.slot)?;
        if monitor.slot != 0 && monitor.epoch != snapshot.epoch {
            return Err(DiscoveryError::Malformed(
                "epoch changed during fee scan; retry".into(),
            ));
        }
        monitor.slot = snapshot.slot;
        monitor.epoch = snapshot.epoch;
        for (mint, phase) in chunk.iter().zip(snapshot.phases) {
            match phase {
                FeePhase::Notice => monitor.notice_mints.push(mint.clone()),
                FeePhase::FinalNotice => monitor.final_notice_mints.push(mint.clone()),
                FeePhase::None => {}
            }
        }
    }
    monitor.final_notice_markets = market_mints
        .into_iter()
        .filter(|(_, mint)| monitor.final_notice_mints.contains(mint))
        .map(|(market, _)| market)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(monitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_adapter::transfer_fees::{SYSVAR_OWNER, TOKEN_2022_PROGRAM};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };
    #[test]
    fn clock_only_rollover_and_cancellation_invalidate_quotes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for (index, epoch) in [10_u64, 11, 11].into_iter().enumerate() {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let count = socket.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&request[..end]);
                        let len: usize = header
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap();
                        if request.len() >= end + 4 + len {
                            let body: serde_json::Value =
                                serde_json::from_slice(&request[end + 4..end + 4 + len]).unwrap();
                            assert_eq!(body["method"], "getMultipleAccounts");
                            assert_eq!(body["params"][0][1], CLOCK);
                            assert_eq!(
                                body["params"][1]["minContextSlot"],
                                if index == 0 { 0 } else { 99 + index }
                            );
                            break;
                        }
                    }
                }
                let mut mint = vec![0; 278];
                mint[45] = 1;
                mint[165] = 1;
                mint[166] = 1;
                mint[168] = 108;
                mint[258..260].copy_from_slice(&300_u16.to_le_bytes());
                mint[250..258].copy_from_slice(&u64::MAX.to_le_bytes());
                mint[260..268].copy_from_slice(&12_u64.to_le_bytes());
                mint[268..276].copy_from_slice(&u64::MAX.to_le_bytes());
                mint[276..278]
                    .copy_from_slice(&(if index == 2 { 300_u16 } else { 10000_u16 }).to_le_bytes());
                let slot = 100 + index as u64;
                let mut clock = vec![0; 40];
                clock[..8].copy_from_slice(&slot.to_le_bytes());
                clock[16..24].copy_from_slice(&epoch.to_le_bytes());
                let account = |owner: &str, data: &[u8]| serde_json::json!({"owner":owner,"executable":false,"data":[crate::transaction::base64(data),"base64"]});
                let body = serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"context":{"slot":slot},"value":[account(TOKEN_2022_PROGRAM,&mint),account(SYSVAR_OWNER,&clock)]}}).to_string();
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let client = RpcClient::new(url, Duration::from_secs(3));
        let keys = vec!["mint".to_string()];
        let first = read_fees(&client, &keys, 0).unwrap();
        let final_notice = read_fees(&client, &keys, first.slot).unwrap();
        let canceled = read_fees(&client, &keys, final_notice.slot).unwrap();
        assert_eq!(first.phases, vec![FeePhase::Notice]);
        assert_eq!(final_notice.phases, vec![FeePhase::FinalNotice]);
        assert_ne!(first.fingerprint, final_notice.fingerprint);
        assert_ne!(final_notice.fingerprint, canceled.fingerprint);
        server.join().unwrap();
    }
}
