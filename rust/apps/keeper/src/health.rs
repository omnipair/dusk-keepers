//! Shared observation state and liveness/readiness endpoints.
use crate::workers::sentinel::{Observation, fee_notice};
use std::{
    collections::BTreeMap,
    error::Error,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
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
    pub last_completed_at: Option<Instant>,
    pub max_scan_age: Duration,
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
        // A disconnected or slow health client must not terminate the keeper.
        if let Ok(stream) = stream {
            let _ = respond(stream, profile, revision, &current);
        }
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
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut buffer = [0_u8; 1024];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = response_body(path, profile, revision, snapshot);
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn response_body(
    path: &str,
    profile: &str,
    revision: &str,
    snapshot: &Snapshot,
) -> (&'static str, String) {
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
    let scan_age = snapshot.last_completed_at.map(|at| at.elapsed());
    let observed = format!(
        "{observed},\"lastCompletedScanAgeMs\":{}",
        scan_age.map_or_else(|| "null".into(), |age| age.as_millis().to_string())
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
    match path {
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
                    scan_age
                        .filter(|age| *age > snapshot.max_scan_age)
                        .map(|_| "last completed scan is stale")
                })
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
    }
}

/// Publishes the same readiness decision as /readyz without error details,
/// credentials, RPC URLs or signing material. A stalled discovery loop still
/// emits degraded/stale reports from this independent thread.
pub(crate) fn spawn_reports(profile: String, revision: String, shared: Arc<Mutex<Snapshot>>) {
    std::thread::spawn(move || {
        loop {
            if let Ok(snapshot) = shared.lock().map(|guard| guard.clone()) {
                println!("{}", report(&profile, &revision, &snapshot));
            }
            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

fn report(profile: &str, revision: &str, snapshot: &Snapshot) -> serde_json::Value {
    let (_, body) = response_body("/readyz", profile, revision, snapshot);
    let ready: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let reason = if snapshot.error.is_some() {
        "discovery failed"
    } else if snapshot.fee_notice_error.is_some() {
        "fee monitoring failed"
    } else if snapshot.last_execution_error.is_some() {
        "execution checks failed"
    } else if snapshot.execution_configuration_error.is_some() {
        "execution configuration invalid"
    } else if !snapshot.first_pass_complete {
        "waiting for first completed pass"
    } else if ready["status"] == "ready" {
        "readiness checks passed"
    } else {
        "scan stale or execution not configured"
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    serde_json::json!({
        "event": "dusk_keeper_health", "schemaVersion": 1,
        "profile": profile, "protocolRevision": revision,
        "observedAtMs": now, "status": ready["status"], "reason": reason,
        "scanCount": snapshot.passes.to_string(), "sentCount": snapshot.sent.to_string(),
        // Sent includes setup. Never claim it proves confirmed protocol execution.
        "confirmedCount": serde_json::Value::Null,
        "lastCompletedScanAgeMs": ready["lastCompletedScanAgeMs"]
    })
}

#[cfg(test)]
mod report_tests {
    use super::*;
    #[test]
    fn report_uses_http_readiness_and_never_turns_setup_into_confirmation() {
        let snapshot = Snapshot {
            first_pass_complete: true,
            execution_configured: true,
            last_completed_at: Some(Instant::now()),
            max_scan_age: Duration::from_secs(60),
            passes: 42,
            sent: 1,
            ..Snapshot::default()
        };
        let value = report("lending", "revision", &snapshot);
        let (_, body) = response_body("/readyz", "lending", "revision", &snapshot);
        assert_eq!(
            value["status"],
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["status"]
        );
        assert_eq!(value["scanCount"], "42");
        assert_eq!(value["sentCount"], "1");
        assert!(value["confirmedCount"].is_null());
    }
    #[test]
    fn failed_stale_starting_and_unconfigured_reports_fail_closed() {
        let mut s = Snapshot::default();
        assert_ne!(report("lending", "rev", &s)["status"], "ready");
        s.first_pass_complete = true;
        s.execution_configured = true;
        s.last_completed_at = Some(Instant::now() - Duration::from_secs(120));
        s.max_scan_age = Duration::from_secs(60);
        assert_eq!(report("lending", "rev", &s)["status"], "degraded");
        s.last_completed_at = Some(Instant::now());
        for field in 0..4 {
            let mut failed = s.clone();
            let error = Some("https://rpc.invalid/?api-key=secret secret=unsafe".into());
            match field {
                0 => failed.error = error,
                1 => failed.fee_notice_error = error,
                2 => failed.last_execution_error = error,
                _ => failed.execution_configuration_error = error,
            }
            let value = report("lending", "rev", &failed);
            assert_eq!(value["status"], "degraded");
            assert!(!value.to_string().contains("secret"));
            assert!(!value.to_string().contains("rpc.invalid"));
        }
    }
    #[test]
    fn sentinel_readiness_has_no_signer_requirement() {
        let s = Snapshot {
            first_pass_complete: true,
            last_completed_at: Some(Instant::now()),
            max_scan_age: Duration::from_secs(60),
            ..Snapshot::default()
        };
        assert_eq!(report("sentinel", "rev", &s)["status"], "ready");
        assert_eq!(report("lending", "rev", &s)["status"], "degraded");
    }
}
