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
    }
}

#[test]
fn mainnet_example_keeps_fee_and_reserve_limits_without_token_caps() {
    let configured: Policy = serde_json::from_str(include_str!(
        "../../../../../../deploy/mainnet/risk-policy.example.json"
    ))
    .unwrap();
    assert_eq!(configured.profiles.len(), 5);
    for profile in configured.profiles.values() {
        profile.validate().unwrap();
        assert_eq!(profile.fees.per_transaction, 200_000);
        assert_eq!(profile.fees.per_day, 20_000_000);
        assert_eq!(profile.minimum_sol_reserve, 50_000_000);
        let claims = profile
            .claims(
                5_000,
                &[TokenDebit {
                    mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
                    maximum: 1_000_000_000,
                }],
            )
            .unwrap();
        assert!(
            claims
                .iter()
                .any(|c| c.maximum == 1_000_000_000 && c.daily_limit.is_none())
        );
    }
}

#[test]
fn inventory_has_no_transaction_cap_or_mint_allowlist_but_fees_stay_capped() {
    let p = policy();
    assert!(p.validate().is_ok());
    let debits = [
        TokenDebit {
            mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
            maximum: 1_000_000_000,
        },
        TokenDebit {
            mint: "So11111111111111111111111111111111111111112".into(),
            maximum: 10_000_000_000,
        },
        TokenDebit {
            mint: crate::liquidation::plan::address([8; 32]),
            maximum: u64::MAX,
        },
    ];
    let claims = p.claims(5_000, &debits).unwrap();
    assert_eq!(claims.len(), 4);
    for claim in claims {
        if claim.asset == FEES {
            assert_eq!(claim.daily_limit, Some(20_000_000));
        } else {
            assert_eq!(claim.daily_limit, None);
            assert_eq!(
                claim.maximum,
                debits
                    .iter()
                    .find(|d| d.mint == claim.asset)
                    .unwrap()
                    .maximum
            );
        }
    }
    assert!(p.claims(200_001, &debits).is_err());
    let debit = TokenDebit {
        mint: crate::liquidation::plan::address([7; 32]),
        maximum: 15_000_000,
    };
    let combined = p.claims(5_000, &[debit.clone(), debit]).unwrap();
    assert!(
        combined
            .iter()
            .any(|c| c.maximum == 30_000_000 && c.daily_limit.is_none())
    );
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
fn fee_budgets_survive_concurrency_and_restart_while_inventory_is_uncapped() {
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
    for work in ["one", "two", "three", "four", "five"] {
        admin.execute("INSERT INTO keeper_flash_work(work_key,revision,market,position,debt_asset,action,observed_slot,state_hash,message,blockhash,last_valid_height) VALUES($1,'release','market',$1,0,'inventory',1,'state','bytes','hash',100)", &[&work]).unwrap();
    }
    let claims = |fee| {
        vec![
            Claim {
                asset: FEES.into(),
                maximum: fee,
                daily_limit: Some(100),
            },
            Claim {
                asset: "token".into(),
                maximum: u64::MAX,
                daily_limit: None,
            },
        ]
    };
    let mut first = connect();
    let mut tx = first.transaction().unwrap();
    reserve(&mut tx, "one", "wallet", &claims(60)).unwrap();
    let mut second = connect();
    let (started, waiting) = std::sync::mpsc::channel();
    let competing = std::thread::spawn(move || {
        let mut tx = second.transaction().unwrap();
        started.send(()).unwrap();
        reserve(&mut tx, "two", "wallet", &claims(60))
    });
    waiting.recv().unwrap();
    tx.commit().unwrap();
    assert!(
        competing
            .join()
            .unwrap()
            .unwrap_err()
            .contains("SOL_FEES daily wallet budget exhausted")
    );
    drop(first);
    let mut restarted = connect();
    restarted.batch_execute("UPDATE keeper_wallet_budget SET created_at=now()-interval '2 days'; UPDATE keeper_flash_work SET outcome='submitted_unknown' WHERE work_key='one'; UPDATE keeper_flash_work SET revision='new-release' WHERE work_key='two'").unwrap();
    let mut tx = restarted.transaction().unwrap();
    assert!(reserve(&mut tx, "two", "wallet", &claims(60)).is_err());
    tx.rollback().unwrap();
    restarted.batch_execute("UPDATE keeper_flash_work SET outcome='abandoned_before_submission' WHERE work_key='one'").unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(&mut tx, "two", "wallet", &claims(60)).unwrap();
    tx.commit().unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(&mut tx, "three", "wallet", &claims(40)).unwrap();
    tx.commit().unwrap();
    // Even a combined token total greater than u64::MAX has no daily cap.
    let total: String = restarted.query_one("SELECT SUM(maximum)::text FROM keeper_wallet_budget WHERE wallet='wallet' AND asset='token' AND work_key IN ('two','three')", &[]).unwrap().get(0);
    assert_eq!(total, (u128::from(u64::MAX) * 2).to_string());
    let mut tx = restarted.transaction().unwrap();
    assert!(reserve(&mut tx, "four", "wallet", &claims(1)).is_err());
    tx.rollback().unwrap();
    // A failed transaction still incurs its fee; an absent one releases it.
    restarted
        .batch_execute("UPDATE keeper_flash_work SET outcome='failed' WHERE work_key='two'")
        .unwrap();
    let mut tx = restarted.transaction().unwrap();
    assert!(reserve(&mut tx, "four", "wallet", &claims(1)).is_err());
    tx.rollback().unwrap();
    restarted
        .batch_execute("UPDATE keeper_flash_work SET outcome='expired_absent' WHERE work_key='two'")
        .unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(&mut tx, "four", "wallet", &claims(1)).unwrap();
    tx.commit().unwrap();
    let mut tx = restarted.transaction().unwrap();
    reserve(&mut tx, "five", "different-wallet", &claims(100)).unwrap();
    tx.commit().unwrap();
    admin
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .unwrap();
}
