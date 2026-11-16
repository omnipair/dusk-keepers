use super::*;

#[test]
fn final_assessment_rejects_stale_quotes_failed_simulations_and_low_sol() {
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::Duration,
    };
    let cases = [
        (100, Some(5_000), 100, 50_010_000, false, true),
        (99, Some(5_000), 100, 50_010_000, false, false),
        (100, None, 100, 50_010_000, false, false),
        (100, Some(200_001), 100, 50_010_000, false, false),
        (100, Some(5_000), 99, 50_010_000, false, false),
        (100, Some(5_000), 100, 50_000_000, false, false),
        (100, Some(5_000), 100, 50_010_000, true, false),
    ];
    for (fee_slot, fee, simulation_slot, balance, rejected, accepted) in cases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let mut replies = vec![(
            "getFeeForMessage",
            json!({"context":{"slot":fee_slot},"value":fee}),
        )];
        if fee_slot == 100 && fee.is_some_and(|f| f <= 200_000) {
            replies.push((
                "simulateTransaction",
                json!({"context":{"slot":simulation_slot}, "value":{
                    "err": if rejected { json!("rejected") } else { json!(null) },
                    "accounts":[{"lamports":balance}]
                }}),
            ));
        }
        let server = std::thread::spawn(move || {
            for (method, result) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let request: serde_json::Value = loop {
                    let mut buffer = [0u8; 4096];
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
                        && let Ok(value) = serde_json::from_slice(&bytes[end + 4..])
                    {
                        break value;
                    }
                };
                assert_eq!(request["method"], method);
                assert_eq!(request["params"][1]["minContextSlot"], 100);
                if method == "simulateTransaction" {
                    assert_eq!(request["params"][1]["replaceRecentBlockhash"], false);
                    assert_eq!(
                        request["params"][1]["accounts"]["addresses"]
                            .as_array()
                            .unwrap()
                            .len(),
                        1
                    );
                }
                let body = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let rpc = RpcClient::new(endpoint, Duration::from_secs(3));
        let result = assess_with_policy(
            &policy(),
            &rpc,
            &crate::liquidation::plan::address([2; 32]),
            b"message",
            100,
            &[],
        );
        assert_eq!(result.is_ok(), accepted, "{result:?}");
        server.join().unwrap();
    }
}

fn policy() -> WalletPolicy {
    WalletPolicy {
        fees: Limits {
            per_transaction: 200_000,
            per_day: 20_000_000,
        },
        minimum_sol_reserve: 50_000_000,
        tokens: BTreeMap::from([(
            crate::liquidation::plan::address([7; 32]),
            Limits {
                per_transaction: 25_000_000,
                per_day: 100_000_000,
            },
        )]),
    }
}

#[test]
fn rejects_unknown_assets_excess_fees_and_combined_payments() {
    let p = policy();
    assert!(p.validate().is_ok());
    assert!(p.claims(5_000, &[]).is_ok());
    assert!(p.claims(200_001, &[]).is_err());
    assert!(
        p.claims(
            5_000,
            &[TokenDebit {
                mint: crate::liquidation::plan::address([8; 32]),
                maximum: 1
            }]
        )
        .is_err()
    );
    let debit = TokenDebit {
        mint: crate::liquidation::plan::address([7; 32]),
        maximum: 15_000_000,
    };
    assert!(p.claims(5_000, std::slice::from_ref(&debit)).is_ok());
    assert!(p.claims(5_000, &[debit.clone(), debit]).is_err());
}

#[test]
fn configuration_preserves_exact_atoms_and_rejects_ambiguous_values() {
    for value in [
        serde_json::json!(1),
        serde_json::json!("-1"),
        serde_json::json!("1.5"),
        serde_json::json!("18446744073709551616"),
    ] {
        assert!(
            serde_json::from_value::<Limits>(
                serde_json::json!({"perTransaction":value,"perDay":"10000000"})
            )
            .is_err()
        );
    }
    let exact: Limits = serde_json::from_value(serde_json::json!({"perTransaction":"18446744073709551615","perDay":"18446744073709551615"})).unwrap();
    assert_eq!(exact.per_day, u64::MAX);
    let mut p = policy();
    p.fees.per_day = 1;
    assert!(p.validate().is_err());
}

#[test]
fn inventory_budget_is_bound_to_the_signed_transfer_amount() {
    use crate::runtime::transaction::{AccountMeta, Instruction};
    let buyer = [2; 32];
    let mut instruction = Instruction {
        program_id: crate::liquidation::plan::key(dusk_adapter::transfer_fees::TOKEN_PROGRAM)
            .unwrap(),
        accounts: vec![
            AccountMeta::writable([3; 32]),
            AccountMeta::readonly([7; 32]),
            AccountMeta::writable([4; 32]),
            AccountMeta::signer(buyer),
        ],
        data: [vec![12], 123u64.to_le_bytes().to_vec(), vec![6]].concat(),
    };
    let claims = inventory_debits(&[instruction.clone()], buyer).unwrap();
    assert_eq!(claims[0].maximum, 123);
    assert_eq!(claims[0].mint, crate::liquidation::plan::address([7; 32]));
    instruction.data[0] = 3;
    assert!(inventory_debits(&[instruction], buyer).is_err());
    assert!(inventory_debits(&[], buyer).is_err());
}

#[test]
#[ignore = "requires isolated KEEPER_TEST_DATABASE_URL"]
fn durable_wallet_budgets_serialize_concurrent_workers_and_survive_restart() {
    use postgres::{Client, NoTls};
    let url = env::var("KEEPER_TEST_DATABASE_URL").expect("isolated test database");
    let schema = format!("risk_test_{}", std::process::id());
    let connect = || {
        let mut client = Client::connect(&url, NoTls).unwrap();
        client
            .batch_execute(&format!("SET search_path TO {schema}"))
            .unwrap();
        client
    };
    let mut admin = Client::connect(&url, NoTls).unwrap();
    admin
        .batch_execute(&format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        ))
        .unwrap();
    admin
        .batch_execute(include_str!(
            "../../../../../../deploy/postgres/migrations/0002_flash_attempts.sql"
        ))
        .unwrap();
    admin
        .batch_execute(include_str!(
            "../../../../../../deploy/postgres/migrations/0004_wallet_budgets.sql"
        ))
        .unwrap();
    for (work, position) in [
        ("one", "one"),
        ("two", "two"),
        ("three", "three"),
        ("four", "four"),
    ] {
        admin.execute("INSERT INTO keeper_flash_work(work_key,revision,market,position,debt_asset,action,observed_slot,state_hash,message,blockhash,last_valid_height) VALUES($1,'release','market',$2,0,'inventory',1,'state','bytes','hash',100)", &[&work, &position]).unwrap();
    }
    let claim = || {
        vec![Claim {
            asset: "token".into(),
            maximum: 60,
            daily_limit: 100,
        }]
    };
    let mut first = connect();
    let mut tx = first.transaction().unwrap();
    reserve(&mut tx, "one", "wallet", &claim()).unwrap();
    let mut second = connect();
    let (started, waiting) = std::sync::mpsc::channel();
    let competing = std::thread::spawn(move || {
        let mut tx = second.transaction().unwrap();
        started.send(()).unwrap();
        reserve(&mut tx, "two", "wallet", &claim())
    });
    waiting.recv().unwrap();
    tx.commit().unwrap();
    assert!(
        competing
            .join()
            .unwrap()
            .unwrap_err()
            .contains("daily wallet budget exhausted")
    );
    drop(first);
    let mut restarted = connect();
    // A pending transaction from yesterday still reserves today's available exposure.
    restarted.batch_execute("UPDATE keeper_wallet_budget SET created_at=now()-interval '2 days'; UPDATE keeper_flash_work SET outcome='submitted_unknown' WHERE work_key='one'").unwrap();
    let mut tx = restarted.transaction().unwrap();
    assert!(reserve(&mut tx, "two", "wallet", &claim()).is_err());
    tx.rollback().unwrap();
    restarted.batch_execute("UPDATE keeper_flash_work SET outcome='abandoned_before_submission' WHERE work_key='one'").unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(&mut tx, "two", "wallet", &claim()).unwrap();
    tx.commit().unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(&mut tx, "three", "different-wallet", &claim()).unwrap();
    tx.commit().unwrap();
    // Failed transactions release token exposure, but their paid fee remains
    // charged. A new revision cannot evade either reservation.
    restarted.execute("INSERT INTO keeper_wallet_budget(work_key,wallet,asset,maximum) VALUES('two','wallet','SOL_FEES',60)", &[]).unwrap();
    restarted.batch_execute("UPDATE keeper_flash_work SET outcome='failed' WHERE work_key='two'; UPDATE keeper_flash_work SET revision='new-release' WHERE work_key='one'").unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(
        &mut tx,
        "one",
        "wallet",
        &[Claim {
            asset: FEES.into(),
            maximum: 60,
            daily_limit: 100,
        }],
    )
    .unwrap_err();
    tx.rollback().unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(
        &mut tx,
        "one",
        "wallet",
        &[Claim {
            asset: "other-token".into(),
            maximum: 60,
            daily_limit: 100,
        }],
    )
    .unwrap();
    tx.rollback().unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(
        &mut tx,
        "four",
        "wallet",
        &[Claim {
            asset: "token".into(),
            maximum: 100,
            daily_limit: 100,
        }],
    )
    .unwrap();
    tx.rollback().unwrap();
    admin
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .unwrap();
}
