use super::*;
use crate::runtime::transaction::base64;
use dusk_adapter::transfer_fees::SYSVAR_OWNER;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

fn rpc(responses: Vec<Value>) -> (RpcClient, thread::JoinHandle<Vec<Value>>) {
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
                    assert_eq!(body["method"], "getMultipleAccounts");
                    requests.push(body);
                    break;
                }
            }
            let body = response.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        requests
    });
    (client, server)
}

fn account(owner: &str, data: &[u8]) -> Value {
    json!({"owner":owner,"executable":false,"lamports":1,"data":[base64(data),"base64"]})
}

fn clock(slot: u64, epoch: u64) -> Value {
    let mut data = [0; 40];
    data[..8].copy_from_slice(&slot.to_le_bytes());
    data[16..24].copy_from_slice(&epoch.to_le_bytes());
    account(SYSVAR_OWNER, &data)
}

fn response(slot: u64, accounts: Vec<Value>) -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{"context":{"slot":slot},"value":accounts}})
}

#[test]
fn clock_retry_replaces_entire_batch_and_preserves_freshness_floor() {
    let keys = [[1; 32], key(CLOCK).unwrap(), [2; 32]];
    let (client, server) = rpc(vec![
        response(
            10,
            vec![account("owner", &[1]), clock(9, 0), account("owner", &[2])],
        ),
        response(
            12,
            vec![account("owner", &[3]), clock(12, 1), account("owner", &[4])],
        ),
    ]);
    let snapshot = Snapshot::fetch(&client, &keys, 8).unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    for (request, min_slot) in requests.iter().zip([8, 10]) {
        assert_eq!(request["params"][0], json!(keys.map(address)));
        assert_eq!(request["params"][1]["minContextSlot"], min_slot);
        assert_eq!(request["params"][1]["commitment"], "confirmed");
    }
    assert_eq!(snapshot.slot, 12);
    assert_eq!(snapshot.account(keys[0]).unwrap().data, [3]);
    assert_eq!(snapshot.account(keys[2]).unwrap().data, [4]);
    let clock = snapshot.account(keys[1]).unwrap();
    assert_eq!(
        decode_clock_epoch(&clock.owner, &clock.data, snapshot.slot).unwrap(),
        1
    );
}

#[test]
fn persistent_clock_mismatch_stops_after_three_reads() {
    let (client, server) = rpc((10..13)
        .map(|slot| response(slot, vec![clock(slot - 1, 0)]))
        .collect());
    let error = Snapshot::fetch(&client, &[key(CLOCK).unwrap()], 8).unwrap_err();
    assert!(
        matches!(error, DiscoveryError::Malformed(ref detail) if detail ==
        "clock and RPC bank do not match: clock_slot=11 rpc_slot=12 attempts=3")
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    for (request, min_slot) in requests.iter().zip([8, 10, 11]) {
        assert_eq!(request["params"][1]["minContextSlot"], min_slot);
    }
}

#[test]
fn matching_clock_needs_one_read() {
    let (client, server) = rpc(vec![response(10, vec![clock(10, 1)])]);
    assert_eq!(
        Snapshot::fetch(&client, &[key(CLOCK).unwrap()], 8)
            .unwrap()
            .slot,
        10
    );
    assert_eq!(server.join().unwrap().len(), 1);
}

#[test]
fn discovery_without_clock_needs_one_read() {
    let (client, server) = rpc(vec![response(10, vec![account("owner", &[1])])]);
    assert_eq!(Snapshot::fetch(&client, &[[1; 32]], 8).unwrap().slot, 10);
    assert_eq!(server.join().unwrap().len(), 1);
}

#[test]
fn malformed_clock_is_rejected_without_retry() {
    let mut wrong_owner = clock(9, 0);
    wrong_owner["owner"] = json!("not the sysvar owner");
    let mut executable = clock(9, 0);
    executable["executable"] = json!(true);
    for invalid in [
        wrong_owner,
        executable,
        Value::Null,
        account(SYSVAR_OWNER, &[0; 7]),
        account(SYSVAR_OWNER, &[0; 39]),
        account(SYSVAR_OWNER, &[0; 41]),
    ] {
        let (client, server) = rpc(vec![response(10, vec![invalid])]);
        assert!(matches!(
            Snapshot::fetch(&client, &[key(CLOCK).unwrap()], 8),
            Err(DiscoveryError::Malformed(_))
        ));
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[test]
fn retry_does_not_accept_a_consistent_but_older_snapshot() {
    let (client, server) = rpc(vec![
        response(10, vec![clock(9, 0)]),
        response(9, vec![clock(9, 0)]),
    ]);
    let error = Snapshot::fetch(&client, &[key(CLOCK).unwrap()], 8).unwrap_err();
    assert!(
        matches!(error, DiscoveryError::Malformed(ref detail) if detail == "stale/incomplete snapshot")
    );
    assert_eq!(server.join().unwrap().len(), 2);
}

#[test]
fn non_transient_rpc_errors_are_not_retried_as_clock_mismatches() {
    let (client, server) = rpc(vec![
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"invalid params"}}),
    ]);
    assert!(matches!(
        Snapshot::fetch(&client, &[key(CLOCK).unwrap()], 8),
        Err(DiscoveryError::Rpc(_))
    ));
    assert_eq!(server.join().unwrap().len(), 1);
}

#[test]
fn slot_errors_between_clock_retries_preserve_the_entire_batch_and_floor() {
    let keys = [[1; 32], key(CLOCK).unwrap()];
    let (client, server) = rpc(vec![
        response(10, vec![account("owner", &[1]), clock(11, 0)]),
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32016,"message":"minimum slot unavailable"}}),
        response(12, vec![account("owner", &[2]), clock(12, 1)]),
    ]);
    let snapshot = Snapshot::fetch(&client, &keys, 8).unwrap();
    assert_eq!(snapshot.slot, 12);
    assert_eq!(snapshot.account(keys[0]).unwrap().data, [2]);
    let clock = snapshot.account(keys[1]).unwrap();
    assert_eq!(
        decode_clock_epoch(&clock.owner, &clock.data, snapshot.slot).unwrap(),
        1
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    for (request, min_slot) in requests.iter().zip([8, 10, 10]) {
        assert_eq!(request["params"][0], json!(keys.map(address)));
        assert_eq!(request["params"][1]["minContextSlot"], min_slot);
        assert_eq!(request["params"][1]["commitment"], "confirmed");
    }
    assert_eq!(requests[1], requests[2]);
}

#[test]
fn slot_retry_does_not_accept_a_consistent_but_older_snapshot() {
    let (client, server) = rpc(vec![
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32016,"message":"minimum slot unavailable"}}),
        response(9, vec![clock(9, 0)]),
    ]);
    let error = Snapshot::fetch(&client, &[key(CLOCK).unwrap()], 10).unwrap_err();
    assert!(
        matches!(error, DiscoveryError::Malformed(ref detail) if detail == "stale/incomplete snapshot")
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[1]["params"][1]["minContextSlot"], 10);
}
