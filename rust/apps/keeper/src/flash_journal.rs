//! Durable single-generation submission. Session advisory locks are released if
//! the DB connection dies; every mutation checks the connection before signing.
use crate::{discovery::RpcClient, signer::TransactionSigner, transaction::serialize_transaction};
use postgres::{Client, NoTls};
use postgres_native_tls::MakeTlsConnector;
use sha2::{Digest, Sha256};
pub struct Journal {
    client: Client,
}
impl Journal {
    pub fn connect(url: &str) -> Result<Self, String> {
        // Local sockets may use NoTls; remote servers require certificate validation.
        let mut config: postgres::Config = url
            .parse::<postgres::Config>()
            .map_err(|_| "invalid DATABASE_URL")?;
        let local = config.get_hosts().iter().all(|h| match h {
            postgres::config::Host::Tcp(h) => h == "127.0.0.1" || h == "localhost" || h == "::1",
            #[cfg(unix)]
            postgres::config::Host::Unix(_) => true,
        });
        let client = if local {
            config.connect(NoTls)
        } else {
            config.ssl_mode(postgres::config::SslMode::Require);
            config.connect(MakeTlsConnector::new(
                native_tls::TlsConnector::builder()
                    .build()
                    .map_err(|e| e.to_string())?,
            ))
        }
        .map_err(|e| e.to_string())?;
        Ok(Self { client })
    }
    /// Reconcile every pending generation before evaluating new work, including
    /// positions which have since closed. Expiry requires complete finalized
    /// block history, not merely a null signature-status response.
    pub fn recover(
        &mut self,
        rpc: &RpcClient,
        revision: &str,
    ) -> Result<Vec<(String, Option<String>, String)>, String> {
        let rows=self.client.query("SELECT work_key,position,debt_asset FROM keeper_flash_work WHERE revision=$1 AND outcome IN ('reserved','submitted_unknown') ORDER BY created_at LIMIT 100",&[&revision]).map_err(|e|e.to_string())?;
        let mut outcomes = Vec::new();
        for row in rows {
            let work: String = row.get(0);
            let position: String = row.get(1);
            let side: i16 = row.get(2);
            let lock_key = format!("{revision}:{position}:{side}");
            let locked: bool = self
                .client
                .query_one(
                    "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
                    &[&lock_key],
                )
                .map_err(|e| e.to_string())?
                .get(0);
            if !locked {
                continue;
            }
            let result = (|| -> Result<Option<(Option<String>, String)>, String> {
                // The discovery row was read before the advisory lock. Re-read every
                // field used for the decision after acquiring it: a submitter may
                // have persisted its signature while recovery waited for the lock.
                let Some(current)=self.client.query_opt("SELECT outcome,signature,observed_slot,last_valid_height FROM keeper_flash_work WHERE work_key=$1",&[&work]).map_err(|e|e.to_string())? else {
                    return Ok(None);
                };
                let outcome: String = current.get(0);
                if outcome != "reserved" && outcome != "submitted_unknown" {
                    return Ok(None);
                }
                let sig: Option<String> = current.get(1);
                let slot: i64 = current.get(2);
                let height: i64 = current.get(3);
                let Some(sig) = sig.as_ref() else {
                    self.finish(&work, "abandoned_before_submission", &serde_json::json!({}))?;
                    return Ok(Some((None, "abandoned_before_submission".into())));
                };
                let statuses: serde_json::Value = rpc
                    .call(
                        "getSignatureStatuses",
                        serde_json::json!([[sig],{"searchTransactionHistory":true}]),
                    )
                    .map_err(|e| e.to_string())?;
                let status = &statuses["value"][0];
                if status["confirmationStatus"] == "finalized" {
                    let outcome = self.finish_receipt(&work, sig, rpc)?;
                    return Ok(Some((Some(sig.clone()), outcome)));
                }
                let final_height: u64 = rpc
                    .call(
                        "getBlockHeight",
                        serde_json::json!([{"commitment":"finalized"}]),
                    )
                    .map_err(|e| e.to_string())?;
                if final_height <= height as u64 {
                    return Ok(None);
                }
                let final_slot: u64 = rpc
                    .call("getSlot", serde_json::json!([{"commitment":"finalized"}]))
                    .map_err(|e| e.to_string())?;
                let start = u64::try_from(slot).map_err(|_| "negative persisted slot")?;
                if final_slot < start || final_slot - start > 100_000 {
                    return Err(
                        "pending signature requires archival reconciliation; refusing replacement"
                            .into(),
                    );
                }
                let mut checked = 0u64;
                let mut saw_tip = false;
                for from in (start..=final_slot).step_by(500) {
                    let to = (from + 499).min(final_slot);
                    let blocks: Vec<u64> = rpc
                        .call(
                            "getBlocks",
                            serde_json::json!([from,to,{"commitment":"finalized"}]),
                        )
                        .map_err(|e| e.to_string())?;
                    if blocks.windows(2).any(|w| w[0] >= w[1])
                        || blocks.iter().any(|b| *b < from || *b > to)
                    {
                        return Err("invalid finalized history range".into());
                    }
                    for block in blocks {
                        saw_tip |= block == final_slot;
                        let value:serde_json::Value=rpc.call("getBlock",serde_json::json!([block,{"commitment":"finalized","transactionDetails":"signatures","rewards":false}])).map_err(|e|e.to_string())?;
                        let signatures = value["signatures"]
                            .as_array()
                            .ok_or("archival history is incomplete; refusing replacement")?;
                        checked += 1;
                        if signatures.iter().any(|s| s.as_str() == Some(sig)) {
                            let outcome = self.finish_receipt(&work, sig, rpc)?;
                            return Ok(Some((Some(sig.clone()), outcome)));
                        }
                    }
                }
                // An empty result for a range ending in a known finalized block is not
                // complete history and must never be treated as proof of absence.
                let tip:serde_json::Value=rpc.call("getBlock",serde_json::json!([final_slot,{"commitment":"finalized","transactionDetails":"signatures","rewards":false}])).map_err(|e|e.to_string())?;
                if checked == 0 || !saw_tip || !tip["signatures"].is_array() {
                    return Err("finalized history coverage unavailable".into());
                }
                self.finish(&work,"expired_absent",&serde_json::json!({"startSlot":start,"finalizedSlot":final_slot,"finalizedBlockHeight":final_height,"checkedBlocks":checked}))?;
                Ok(Some((Some(sig.clone()), "expired_absent".into())))
            })();
            self.client
                .execute(
                    "SELECT pg_advisory_unlock(hashtextextended($1,0))",
                    &[&lock_key],
                )
                .map_err(|e| e.to_string())?;
            if let Some((signature, outcome)) = result? {
                outcomes.push((position, signature, outcome));
            }
        }
        Ok(outcomes)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &mut self,
        rpc: &RpcClient,
        signer: &dyn TransactionSigner,
        revision: &str,
        position: &str,
        debt_asset: u8,
        market: &str,
        action: &str,
        slot: u64,
        state_hash: &str,
        message: &[u8],
        blockhash: &str,
        last_valid_height: u64,
    ) -> Result<Option<String>, String> {
        let lock_key = format!("{revision}:{position}:{debt_asset}");
        let locked: bool = self
            .client
            .query_one(
                "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
                &[&lock_key],
            )
            .map_err(|e| e.to_string())?
            .get(0);
        if !locked {
            return Err("liquidation lease is contended".into());
        }
        let result = self.submit_locked(
            rpc,
            signer,
            revision,
            position,
            debt_asset,
            market,
            action,
            slot,
            state_hash,
            message,
            blockhash,
            last_valid_height,
        );
        let unlock = self.client.execute(
            "SELECT pg_advisory_unlock(hashtextextended($1,0))",
            &[&lock_key],
        );
        if unlock.is_err() {
            return Err("liquidation lease connection lost".into());
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn submit_locked(
        &mut self,
        rpc: &RpcClient,
        signer: &dyn TransactionSigner,
        revision: &str,
        position: &str,
        debt_asset: u8,
        market: &str,
        action: &str,
        slot: u64,
        state_hash: &str,
        message: &[u8],
        blockhash: &str,
        last_valid_height: u64,
    ) -> Result<Option<String>, String> {
        let side = i16::from(debt_asset);
        let pending=self.client.query_opt("SELECT work_key, signature, transaction_bytes, observed_slot, last_valid_height FROM keeper_flash_work WHERE revision=$1 AND position=$2 AND debt_asset=$3 AND outcome IN ('reserved','submitted_unknown')",&[&revision,&position,&side]).map_err(|e|e.to_string())?;
        if let Some(row) = pending {
            let work: String = row.get(0);
            let sig: Option<String> = row.get(1);
            if let Some(sig) = sig {
                let statuses: serde_json::Value = rpc
                    .call(
                        "getSignatureStatuses",
                        serde_json::json!([[sig],{"searchTransactionHistory":true}]),
                    )
                    .map_err(|e| e.to_string())?;
                let status = &statuses["value"][0];
                if status["confirmationStatus"] == "finalized" {
                    let outcome = self.finish_receipt(&work, &sig, rpc)?;
                    return Ok(if outcome == "executed" {
                        Some(sig)
                    } else {
                        None
                    });
                }
                // Re-broadcast only the exact recorded bytes. A lost RPC response never
                // authorizes a new signature or refreshed blockhash.
                let bytes: Vec<u8> = row.get(2);
                let _ = rpc.send_transaction(&crate::transaction::base64(&bytes));
                return Err(format!(
                    "pending signature {sig}; reconcile finalized history before replacement"
                ));
            }
            // Reserved but not submitted has no transaction bytes anywhere; signing
            // occurs only after this same DB transaction records its signature. A
            // crash here is safely canceled before fresh revalidation next pass.
            self.finish(&work, "abandoned_before_submission", &serde_json::json!({}))?;
            return Err("unsubmitted generation canceled; fresh evaluation required".into());
        }
        let work = format!(
            "{:x}",
            Sha256::digest(
                [
                    revision.as_bytes(),
                    position.as_bytes(),
                    &[debt_asset],
                    message
                ]
                .concat()
            )
        );
        let slot = i64::try_from(slot).map_err(|_| "slot overflow")?;
        let height = i64::try_from(last_valid_height).map_err(|_| "height overflow")?;
        let mut tx = self.client.transaction().map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO keeper_flash_work(work_key,revision,market,position,debt_asset,action,observed_slot,state_hash,message,blockhash,last_valid_height) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",&[&work,&revision,&market,&position,&side,&action,&slot,&state_hash,&message,&blockhash,&height]).map_err(|e|e.to_string())?;
        tx.execute(
            "INSERT INTO keeper_flash_events(work_key,phase,evidence) VALUES($1,'reserved',$2)",
            &[
                &work,
                &serde_json::json!({"generation":1,"slot":slot,"stateHash":state_hash}),
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        let signature = signer.sign(message);
        let sig = bs58::encode(signature).into_string();
        let bytes = serialize_transaction(&[signature], message);
        let mut tx = self.client.transaction().map_err(|e| e.to_string())?;
        tx.execute("UPDATE keeper_flash_work SET signature=$2,transaction_bytes=$3,outcome='submitted_unknown',updated_at=now() WHERE work_key=$1",&[&work,&sig,&bytes]).map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO keeper_flash_events(work_key,phase,evidence) VALUES($1,'submitted_unknown',$2)",&[&work,&serde_json::json!({"signature":sig})]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        let returned = rpc
            .send_transaction(&crate::transaction::base64(&bytes))
            .map_err(|e| e.to_string())?;
        if returned != sig {
            return Err("RPC returned a different signature".into());
        }
        Ok(None)
    }
    fn finish_receipt(
        &mut self,
        work: &str,
        signature: &str,
        rpc: &RpcClient,
    ) -> Result<String, String> {
        let receipt: serde_json::Value = rpc.call("getTransaction", serde_json::json!([signature, {
            "commitment":"finalized", "encoding":"base64", "maxSupportedTransactionVersion":0
        }])).map_err(|e| e.to_string())?;
        if !receipt["meta"].is_object()
            || receipt["slot"].as_u64().is_none()
            || receipt["transaction"][1] != "base64"
            || receipt["meta"].get("err").is_none()
        {
            return Err("complete finalized receipt unavailable".into());
        }
        let actual =
            crate::discovery::decode_base64(receipt["transaction"][0].as_str().unwrap_or(""))
                .ok_or("invalid finalized transaction bytes")?;
        let expected: Vec<u8> = self
            .client
            .query_one(
                "SELECT transaction_bytes FROM keeper_flash_work WHERE work_key=$1",
                &[&work],
            )
            .map_err(|e| e.to_string())?
            .get(0);
        if actual != expected {
            return Err("finalized transaction differs from persisted generation".into());
        }
        let outcome = if receipt["meta"]["err"].is_null() {
            "executed"
        } else {
            "failed"
        };
        self.finish(
            work,
            outcome,
            &serde_json::json!({"slot":receipt["slot"],"meta":receipt["meta"]}),
        )?;
        Ok(outcome.into())
    }
    fn finish(
        &mut self,
        work: &str,
        outcome: &str,
        evidence: &serde_json::Value,
    ) -> Result<(), String> {
        let mut tx = self.client.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE keeper_flash_work SET outcome=$2,updated_at=now() WHERE work_key=$1",
            &[&work, &outcome],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO keeper_flash_events(work_key,phase,evidence) VALUES($1,$2,$3)",
            &[&work, &outcome, &evidence],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    struct CountingSigner(Arc<AtomicUsize>);
    impl TransactionSigner for CountingSigner {
        fn public_key(&self) -> [u8; 32] {
            [7; 32]
        }
        fn sign(&self, _: &[u8]) -> [u8; 64] {
            self.0.fetch_add(1, Ordering::SeqCst);
            [9; 64]
        }
    }
    #[test]
    #[ignore = "requires isolated KEEPER_TEST_DATABASE_URL and localhost RPC"]
    fn persists_before_send_reconciles_and_holds_the_distributed_lease() {
        let url = std::env::var("KEEPER_TEST_DATABASE_URL").expect("test database URL");
        let mut journal = Journal::connect(&url).unwrap();
        let schema = format!("flash_test_{}", std::process::id());
        journal
            .client
            .batch_execute(&format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            ))
            .unwrap();
        journal
            .client
            .batch_execute(include_str!(
                "../../../../deploy/postgres/migrations/0002_flash_attempts.sql"
            ))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let phase = Arc::new(AtomicUsize::new(0));
        let mode = phase.clone();
        let handle = std::thread::spawn(move || {
            let mut latest_transaction = String::new();
            for mut stream in listener.incoming().flatten() {
                let mut raw = Vec::new();
                let mut b = [0u8; 4096];
                let request = loop {
                    let n = stream.read(&mut b).unwrap();
                    if n == 0 {
                        break None;
                    }
                    raw.extend(&b[..n]);
                    if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&raw[..end]);
                        let length = header
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if raw.len() >= end + 4 + length {
                            break Some(
                                serde_json::from_slice::<serde_json::Value>(
                                    &raw[end + 4..end + 4 + length],
                                )
                                .unwrap(),
                            );
                        }
                    }
                };
                let Some(req) = request else {
                    continue;
                };
                let method = req["method"].as_str().unwrap();
                if method == "stop" {
                    break;
                }
                let result = match method {
                    "sendTransaction" => {
                        latest_transaction = req["params"][0].as_str().unwrap().to_string();
                        serde_json::json!(bs58::encode([9u8; 64]).into_string())
                    }
                    "getTransaction" if mode.load(Ordering::SeqCst) == 3 => serde_json::Value::Null,
                    "getTransaction" => {
                        serde_json::json!({"slot":15,"meta":{"err":null},"transaction":[latest_transaction,"base64"]})
                    }
                    "getSignatureStatuses" => {
                        if [1, 3].contains(&mode.load(Ordering::SeqCst)) {
                            serde_json::json!({"value":[{"confirmationStatus":"finalized","err":null,"slot":15}]})
                        } else {
                            serde_json::json!({"value":[null]})
                        }
                    }
                    "getBlockHeight" => serde_json::json!(101),
                    "getSlot" => serde_json::json!(20),
                    "getBlocks" => serde_json::json!([10, 20]),
                    "getBlock" if mode.load(Ordering::SeqCst) == 4 => serde_json::Value::Null,
                    "getBlock" => serde_json::json!({"signatures":[]}),
                    _ => panic!("unexpected RPC {method}"),
                };
                let body = serde_json::json!({"jsonrpc":"2.0","id":1,"result":result}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let rpc = RpcClient::new(&endpoint, Duration::from_secs(3));
        let count = Arc::new(AtomicUsize::new(0));
        let signer = CountingSigner(count.clone());
        let submit = |j: &mut Journal, message: &[u8]| {
            j.submit(
                &rpc, &signer, "test", "position", 1, "market", "flash", 10, "state", message,
                "hash", 100,
            )
        };
        let mut competing = Journal::connect(&url).unwrap();
        competing
            .client
            .batch_execute(&format!("SET search_path TO {schema}"))
            .unwrap();
        competing
            .client
            .execute(
                "SELECT pg_advisory_lock(hashtextextended($1,0))",
                &[&"test:position:1"],
            )
            .unwrap();
        assert!(
            submit(&mut journal, b"first")
                .unwrap_err()
                .contains("contended")
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        competing
            .client
            .execute(
                "SELECT pg_advisory_unlock(hashtextextended($1,0))",
                &[&"test:position:1"],
            )
            .unwrap();
        assert_eq!(submit(&mut journal, b"first").unwrap(), None);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(
            submit(&mut competing, b"replacement")
                .unwrap_err()
                .contains("pending signature")
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let row = journal
            .client
            .query_one("SELECT message,outcome FROM keeper_flash_work", &[])
            .unwrap();
        assert_eq!(row.get::<_, Vec<u8>>(0), b"first");
        assert_eq!(row.get::<_, String>(1), "submitted_unknown");
        phase.store(3, Ordering::SeqCst);
        assert!(competing.recover(&rpc, "test").is_err());
        let pending: String = journal
            .client
            .query_one("SELECT outcome FROM keeper_flash_work", &[])
            .unwrap()
            .get(0);
        assert_eq!(pending, "submitted_unknown");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        phase.store(1, Ordering::SeqCst);
        let recovered = competing.recover(&rpc, "test").unwrap();
        assert_eq!(recovered[0].2, "executed");
        phase.store(2, Ordering::SeqCst);
        assert_eq!(submit(&mut journal, b"second").unwrap(), None);
        phase.store(4, Ordering::SeqCst);
        assert!(journal.recover(&rpc, "test").is_err());
        phase.store(2, Ordering::SeqCst);
        let recovered = journal.recover(&rpc, "test").unwrap();
        assert_eq!(recovered[0].2, "expired_absent");
        let phases: Vec<String> = journal
            .client
            .query(
                "SELECT phase FROM keeper_flash_events ORDER BY sequence",
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        assert_eq!(
            phases,
            vec![
                "reserved",
                "submitted_unknown",
                "executed",
                "reserved",
                "submitted_unknown",
                "expired_absent"
            ]
        );
        let _ = rpc.call::<serde_json::Value>("stop", serde_json::json!([]));
        handle.join().unwrap();
        journal
            .client
            .batch_execute(&format!(
                "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
            ))
            .unwrap();
    }
}
