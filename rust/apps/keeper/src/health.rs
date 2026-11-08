//! Shared observation state and liveness/readiness endpoints.
use crate::workers::sentinel::{Observation, fee_notice};
use std::{
    collections::BTreeMap,
    error::Error,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
};
/// What the last discovery pass saw, shared with the health endpoint.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    pub observation: Observation,
    pub fee_notices: Option<fee_notice::NoticeMonitor>,
    pub fee_notice_error: Option<String>,
    pub error: Option<String>,
    pub passes: u64,
    /// Readiness waits for discovery, fee monitoring, and execution checks.
    pub first_pass_complete: bool,
    /// Execution counters. Sent is deliberately separate from evaluated: on a
    /// healthy market the first climbs and the second does not, and an
    /// operator needs to see that the keeper is working rather than idle.
    pub evaluated: u64,
    pub execution_passes: u64,
    pub execution_configured: bool,
    pub execution_configuration_error: Option<String>,
    pub sent: u64,
    pub last_signature: Option<String>,
    pub last_execution_error: Option<String>,
    /// The last skip's own words. A reason code says a bound was not met; it
    /// does not say which bound or by how much, and an operator looking at a
    /// keeper that never acts needs the second thing.
    pub last_skip_detail: Option<String>,
    /// Why the last pass declined to act, counted by reason.
    ///
    /// Without this, "every position is healthy" and "every simulation is
    /// failing because the keeper builds a malformed transaction" both read as
    /// zero sends. They are the two most important states to tell apart.
    pub last_reasons: BTreeMap<String, u32>,
}

pub(crate) fn serve(
    address: SocketAddr,
    profile: &str,
    revision: &str,
    snapshot: Arc<Mutex<Snapshot>>,
) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(address)?;
    for stream in listener.incoming() {
        let current = snapshot
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        respond(stream?, profile, revision, &current)?;
    }
    Ok(())
}

fn optional_json(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".to_owned(),
        |text| serde_json::Value::String(text.to_owned()).to_string(),
    )
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
        "\"slot\":{},\"markets\":{},\"borrowPositions\":{},\"leveragePositions\":{},\"passes\":{},\"evaluated\":{},\"sent\":{},\"lastSignature\":{},\"executionError\":{}",
        snapshot.observation.slot,
        snapshot.observation.markets,
        snapshot.observation.borrow_positions,
        snapshot.observation.leverage_positions,
        snapshot.passes,
        snapshot.evaluated,
        snapshot.sent,
        optional_json(snapshot.last_signature.as_deref()),
        optional_json(snapshot.last_execution_error.as_deref()),
    );
    let observed = format!(
        "{observed},\"lastSkipDetail\":{}",
        optional_json(snapshot.last_skip_detail.as_deref())
    );
    let observed = format!(
        "{observed},\"executionConfigured\":{},\"executionPasses\":{},\"firstPassComplete\":{}",
        snapshot.execution_configured, snapshot.execution_passes, snapshot.first_pass_complete
    );
    let reasons = serde_json::to_string(&snapshot.last_reasons).unwrap_or_else(|_| "{}".to_owned());
    let observed = format!("{observed},\"lastReasons\":{reasons}");
    let notices = serde_json::to_string(&snapshot.fee_notices).unwrap_or_else(|_| "null".into());
    let observed = format!(
        "{observed},\"feeNotices\":{notices},\"feeNoticeError\":{}",
        optional_json(snapshot.fee_notice_error.as_deref())
    );
    let (status, body) = match path {
        // Liveness is about the process; readiness is about whether this
        // keeper can currently see the chain, so a discovery failure fails
        // readiness closed while liveness stays healthy.
        "/healthz" => (
            "200 OK",
            format!(
                "{{\"status\":\"healthy\",\"profile\":\"{profile}\",\"protocolRevision\":\"{revision}\",{observed}}}"
            ),
        ),
        "/readyz" => {
            let blocked = snapshot
                .error
                .as_deref()
                .or(snapshot.fee_notice_error.as_deref())
                .or(snapshot.last_execution_error.as_deref())
                .or(snapshot.execution_configuration_error.as_deref())
                .or_else(|| {
                    (profile != "sentinel" && !snapshot.execution_configured)
                        .then_some("execution is not configured")
                });
            match blocked {
                Some(reason) => (
                    "503 Service Unavailable",
                    format!(
                        "{{\"status\":\"degraded\",\"profile\":\"{profile}\",\"protocolRevision\":\"{revision}\",\"reason\":{},{observed}}}",
                        serde_json::Value::String(reason.to_owned())
                    ),
                ),
                None if !snapshot.first_pass_complete => (
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
