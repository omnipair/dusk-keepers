use std::{
    env,
    error::Error,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
};

use std::{
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use dusk_adapter::ProtocolLock;

mod discovery;

use discovery::{observe, Observation, RpcClient};

/// What the last discovery pass saw, shared with the health endpoint.
#[derive(Clone, Debug, Default)]
struct Snapshot {
    observation: Observation,
    error: Option<String>,
    passes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Shadow,
    Live,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "shadow" => Ok(Self::Shadow),
            "live" => Ok(Self::Live),
            _ => Err(format!("KEEPER_MODE must be shadow or live, got {value}")),
        }
    }
}

#[derive(Debug)]
struct Config {
    mode: Mode,
    profile: String,
    bind_address: SocketAddr,
    protocol_lock: PathBuf,
}

impl Config {
    fn from_environment() -> Result<Self, Box<dyn Error>> {
        let mode = Mode::parse(&env::var("KEEPER_MODE").unwrap_or_else(|_| "shadow".into()))?;
        let profile = env::var("KEEPER_PROFILE").unwrap_or_else(|_| "conformance".into());
        let port = env::var("PORT").unwrap_or_else(|_| "8080".into());
        let bind_address = format!("0.0.0.0:{port}").parse()?;
        let protocol_lock = env::var("DUSK_PROTOCOL_LOCK")
            .unwrap_or_else(|_| "protocol.lock.json".into())
            .into();
        Ok(Self {
            mode,
            profile,
            bind_address,
            protocol_lock,
        })
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::from_environment()?;
    let lock = ProtocolLock::read(&config.protocol_lock)?;

    if config.mode == Mode::Live {
        lock.assert_live_ready()?;
        return Err("live executor is intentionally not implemented in this scaffold".into());
    }

    println!(
        "keeper profile={} mode=shadow protocol_revision={}",
        config.profile, lock.revision
    );
    if env::args().any(|argument| argument == "--check") {
        return Ok(());
    }

    let dusk_program_id = lock
        .programs
        .iter()
        .find(|program| program.name == "dusk")
        .and_then(|program| program.program_id.clone())
        .ok_or("protocol lock does not pin a dusk program id")?;

    let snapshot = Arc::new(Mutex::new(Snapshot::default()));

    // Discovery runs even in shadow mode: reading is how the sentinel earns
    // trust before any profile is given a key.
    if let Ok(endpoint) = env::var("SOLANA_RPC_HTTP_URL") {
        let interval = env::var("DISCOVERY_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(15_000);
        let shared = Arc::clone(&snapshot);
        thread::spawn(move || {
            let client = RpcClient::new(endpoint, Duration::from_secs(20));
            loop {
                let result = observe(&client, &dusk_program_id);
                if let Ok(mut current) = shared.lock() {
                    current.passes += 1;
                    match result {
                        Ok(observation) => {
                            current.observation = observation;
                            current.error = None;
                        }
                        // A failed pass keeps the last observation and records
                        // why, so readiness can distinguish "nothing to do"
                        // from "cannot see".
                        Err(error) => current.error = Some(error.to_string()),
                    }
                }
                thread::sleep(Duration::from_millis(interval));
            }
        });
    }

    serve(config.bind_address, &config.profile, &lock.revision, snapshot)
}

fn serve(
    address: SocketAddr,
    profile: &str,
    revision: &str,
    snapshot: Arc<Mutex<Snapshot>>,
) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(address)?;
    for stream in listener.incoming() {
        let current = snapshot.lock().map(|guard| guard.clone()).unwrap_or_default();
        respond(stream?, profile, revision, &current)?;
    }
    Ok(())
}

fn respond(
    mut stream: TcpStream,
    profile: &str,
    revision: &str,
    snapshot: &Snapshot,
) -> std::io::Result<()> {
    let mut buffer = [0_u8; 1024];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let observed = format!(
        "\"slot\":{},\"markets\":{},\"borrowPositions\":{},\"leveragePositions\":{},\"passes\":{}",
        snapshot.observation.slot,
        snapshot.observation.markets,
        snapshot.observation.borrow_positions,
        snapshot.observation.leverage_positions,
        snapshot.passes,
    );
    let (status, body) = match path {
        // Liveness is about the process; readiness is about whether this
        // keeper can currently see the chain, so a discovery failure fails
        // readiness closed while liveness stays healthy.
        "/healthz" => (
            "200 OK",
            format!(
                "{{\"status\":\"healthy\",\"mode\":\"shadow\",\"profile\":\"{profile}\",\"protocolRevision\":\"{revision}\",{observed}}}"
            ),
        ),
        "/readyz" => {
            let blocked = snapshot.error.as_deref();
            match blocked {
                Some(reason) => (
                    "503 Service Unavailable",
                    format!(
                        "{{\"status\":\"degraded\",\"profile\":\"{profile}\",\"protocolRevision\":\"{revision}\",\"reason\":{},{observed}}}",
                        serde_json::Value::String(reason.to_owned())
                    ),
                ),
                None if snapshot.passes == 0 => (
                    "503 Service Unavailable",
                    format!(
                        "{{\"status\":\"starting\",\"profile\":\"{profile}\",\"protocolRevision\":\"{revision}\",{observed}}}"
                    ),
                ),
                None => (
                    "200 OK",
                    format!(
                        "{{\"status\":\"ready\",\"profile\":\"{profile}\",\"protocolRevision\":\"{revision}\",{observed}}}"
                    ),
                ),
            }
        }
        _ => ("404 Not Found", "{\"status\":\"not_found\"}".into()),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_is_fail_closed() {
        assert_eq!(Mode::parse("shadow").unwrap(), Mode::Shadow);
        assert_eq!(Mode::parse("live").unwrap(), Mode::Live);
        assert!(Mode::parse("send").is_err());
    }
}
