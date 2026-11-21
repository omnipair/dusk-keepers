use super::*;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Instant,
};

// None closes the connection after receiving the request, without a response.
fn rpc(responses: Vec<Option<Value>>) -> (RpcClient, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let client = RpcClient::new(
        format!("http://{}", listener.local_addr().unwrap()),
        Duration::from_secs(2),
    );
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "expected an RPC request");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            };
            // macOS may inherit the nonblocking listener flag on accepted sockets.
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n")
                    && let Ok(body) = serde_json::from_slice::<Value>(&request[end + 4..])
                {
                    requests.push(body);
                    break;
                }
            }
            if let Some(response) = response {
                let body = response.to_string();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        }
        requests
    });
    (client, server)
}

fn slot_error() -> Value {
    json!({"jsonrpc":"2.0","id":1,"error":{
        "code":-32016,"message":"Minimum context slot has not been reached",
        "data":{"contextSlot":99}
    }})
}

#[test]
fn transient_slot_errors_retry_identical_reads_and_simulations() {
    let config = json!({"commitment":"confirmed","minContextSlot":100,"encoding":"base64"});
    for (method, params) in [
        (
            "getMultipleAccounts",
            json!([["account-a", "account-b"], config]),
        ),
        ("getAccountInfo", json!(["account-a", config])),
        ("getLatestBlockhash", json!([config])),
        ("getFeeForMessage", json!(["unsigned-message", config])),
        (
            "simulateTransaction",
            json!(["unsigned-transaction",{
                "commitment":"confirmed","minContextSlot":100,"encoding":"base64",
                "sigVerify":false,"replaceRecentBlockhash":true,
                "accounts":{"encoding":"base64","addresses":["account-a"]}
            }]),
        ),
    ] {
        let result = json!({"context":{"slot":101},"value":[]});
        let (client, server) = rpc(vec![
            Some(slot_error()),
            // Provider wording may differ: the numeric code controls retry.
            Some(json!({"error":{"code":-32016,"message":"slot unavailable"}})),
            Some(json!({"result":result})),
        ]);
        assert_eq!(
            client.call::<Value>(method, params.clone()).unwrap(),
            result
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3, "{method}");
        for request in &requests {
            assert_eq!(request["method"], method);
            assert_eq!(request["params"], params);
            assert_eq!(request, &requests[0]);
        }
    }
}

#[test]
fn persistent_slot_errors_stop_after_four_attempts_with_backoff() {
    let (client, server) = rpc(vec![Some(slot_error()); 4]);
    let start = Instant::now();
    let error = client
        .call::<Value>("getMultipleAccounts", json!([[],{"minContextSlot":100}]))
        .unwrap_err();
    assert!(matches!(error, DiscoveryError::Rpc(ref message)
        if message == "Minimum context slot has not been reached"));
    // Check the total lower bound, without an upper bound sensitive to CI load.
    assert!(start.elapsed() >= Duration::from_millis(150 + 300 + 600));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests.iter().all(|request| request == &requests[0]));
}

#[test]
fn transport_and_slot_errors_share_a_bounded_attempt_budget() {
    let (client, server) = rpc(vec![
        Some(slot_error()),
        None,
        Some(slot_error()),
        Some(slot_error()),
    ]);
    let error = client
        .call::<Value>("getLatestBlockhash", json!([{"minContextSlot":100}]))
        .unwrap_err();
    assert!(matches!(error, DiscoveryError::Rpc(_)));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests.iter().all(|request| request == &requests[0]));
}

#[test]
fn other_rpc_errors_and_missing_codes_are_not_retried() {
    for error in [
        json!({"code":-32602,"message":"invalid params"}),
        json!({"code":-32002,"message":"transaction simulation failed"}),
        json!({"code":-32603,"message":"Minimum context slot has not been reached"}),
        json!({"message":"Minimum context slot has not been reached"}),
    ] {
        let (client, server) = rpc(vec![Some(json!({"error":error}))]);
        let result = client.call::<Value>("simulateTransaction", json!([]));
        assert!(matches!(result, Err(DiscoveryError::Rpc(ref message))
            if message == error["message"].as_str().unwrap()));
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[test]
fn slot_error_does_not_retry_transaction_submission() {
    let (client, server) = rpc(vec![Some(slot_error())]);
    assert!(matches!(
        client.send_transaction("test-transaction"),
        Err(DiscoveryError::Rpc(_))
    ));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["method"], "sendTransaction");
    assert_eq!(requests[0]["params"][0], "test-transaction");
}

#[test]
fn simulation_execution_failure_is_returned_without_retry() {
    let result = json!({"context":{"slot":100},"value":{"err":"AccountNotFound"}});
    let (client, server) = rpc(vec![Some(json!({"result":result}))]);
    assert_eq!(
        client
            .call::<Value>("simulateTransaction", json!([]))
            .unwrap(),
        result
    );
    assert_eq!(server.join().unwrap().len(), 1);
}
