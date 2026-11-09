use super::*;
use crate::{
    liquidation::plan::key,
    runtime::{rpc::decode_base64, transaction::base64},
};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("build-fixture.json")).unwrap()
}
fn request() -> Request {
    let f = fixture();
    let a = &f["swapInstruction"]["accounts"];
    let k = |i: usize| key(a[i]["pubkey"].as_str().unwrap()).unwrap();
    Request {
        buyer: k(0),
        source: k(1),
        destination: k(2),
        input_mint: k(3),
        output_mint: k(4),
        input_program: k(5),
        output_program: k(6),
        amount: 10_000_000,
        protected: Default::default(),
    }
}
fn validate(value: Value) -> Result<ValidatedRoute> {
    validation::validate(serde_json::from_value(value).unwrap(), &request(), 50)
}
#[test]
fn captured_public_build_matches_onchain_idl_and_binary_bounds() {
    let route = validate(fixture()).unwrap();
    assert_eq!(route.minimum_output, 887843); // conservative floor, API uses ceil
    assert!(!route.tables.is_empty());
    let idl: Value = serde_json::from_str(include_str!("aggregator-idl.json")).unwrap();
    let layout = idl["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "route_v2")
        .unwrap();
    assert_eq!(layout["discriminator"], json!(route.swap.data[..8]));
    assert_eq!(
        layout["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["type"].clone())
            .take(5)
            .collect::<Vec<_>>(),
        vec![
            json!("u64"),
            json!("u64"),
            json!("u16"),
            json!("u16"),
            json!("u16")
        ]
    );
    assert_eq!(layout["accounts"][0]["name"], "user_transfer_authority");
    assert_eq!(layout["accounts"][1]["name"], "user_source_token_account");
    assert_eq!(
        layout["accounts"][2]["name"],
        "user_destination_token_account"
    );
}
#[test]
fn rejects_changed_quote_identity_and_unsafe_instruction_groups() {
    for (pointer, value) in [
        ("/inputMint", json!(address([1; 32]))),
        ("/outputMint", json!(address([2; 32]))),
        ("/inAmount", json!("9999999")),
        ("/outAmount", json!("18446744073709551616")),
        ("/otherAmountThreshold", json!("9999999")),
        ("/slippageBps", json!(51)),
        ("/swapMode", json!("ExactOut")),
        ("/transactionVersion", json!(1)),
        ("/swapInstruction/programId", json!(address([3; 32]))),
        (
            "/swapInstruction/accounts/1/pubkey",
            json!(address([4; 32])),
        ),
        (
            "/swapInstruction/accounts/2/pubkey",
            json!(address([5; 32])),
        ),
        ("/swapInstruction/accounts/0/isWritable", json!(true)),
        ("/swapInstruction/accounts/3/isSigner", json!(true)),
        ("/setupInstructions/0/data", json!("AA==")),
    ] {
        let mut f = fixture();
        *f.pointer_mut(pointer).unwrap() = value;
        assert!(validate(f).is_err(), "accepted {pointer}");
    }
    for field in ["cleanupInstruction", "tipInstruction", "otherInstructions"] {
        let mut f = fixture();
        let ix = f["setupInstructions"][0].clone();
        f[field] = if field == "otherInstructions" {
            json!([ix])
        } else {
            ix
        };
        assert!(validate(f).is_err());
    }
}
#[test]
fn rejects_binary_bounds_tampering_even_when_quote_json_is_unchanged() {
    for offset in [0, 8, 16, 24, 26, 28, 30] {
        let mut f = fixture();
        let mut bytes = decode_base64(f["swapInstruction"]["data"].as_str().unwrap()).unwrap();
        bytes[offset] = 255;
        f["swapInstruction"]["data"] = base64(&bytes).into();
        assert!(validate(f).is_err(), "accepted changed byte {offset}");
    }
    let mut r = request();
    r.protected.insert(r.source);
    assert!(validation::validate(serde_json::from_value(fixture()).unwrap(), &r, 50).is_err());
}
#[test]
fn error_responses_never_turn_outages_into_no_route() {
    assert!(
        classify_error::<()>(400, br#"{"error":"No routes found"}"#)
            .unwrap()
            .is_none()
    );
    for status in [401, 403, 404, 429, 500, 503] {
        assert!(classify_error::<()>(status, br#"{"error":"No routes found"}"#).is_err());
    }
    for body in [b"not json".as_slice(), br#"{"error":"invalid mint"}"#] {
        assert!(classify_error::<()>(400, body).is_err());
    }
}
#[test]
fn http_client_sends_exact_in_controls_and_preserves_typed_errors() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/build", server.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = server.accept().unwrap();
        let mut data = [0u8; 8192];
        let len = socket.read(&mut data).unwrap();
        let request = std::str::from_utf8(&data[..len]).unwrap();
        for parameter in [
            "amount=10000000",
            "transactionVersion=0",
            "wrapAndUnwrapSol=false",
            "slippageBps=50",
            "maxAccounts=24",
            "destinationTokenAccount=",
        ] {
            assert!(request.contains(parameter), "missing {parameter}");
        }
        let body = serde_json::to_string(&fixture()).unwrap();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let config = Config {
        slippage_bps: 50,
        max_accounts: 24,
        attempts: 4,
        max_age: Duration::from_secs(10),
        priority_lamports: 0,
        max_sol_debit: 20_000_000,
        timeout: Duration::from_secs(2),
        interval: Duration::ZERO,
        api_key: None,
    };
    let mut client = Client::new(&config);
    client.endpoint = endpoint;
    let mut budget = 1;
    let route = client
        .build(&config, &request(), 24, &mut budget)
        .unwrap()
        .unwrap();
    assert!(route.expires_at > Instant::now());
    assert_eq!(budget, 0);
    assert!(client.build(&config, &request(), 24, &mut budget).is_err());
    thread.join().unwrap();
}
