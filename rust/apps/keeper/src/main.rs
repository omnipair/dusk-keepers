use std::{
    collections::BTreeMap,
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

use dusk_adapter::{
    AccountLayoutManifest, AccountResolutionManifest, DeterministicAccountResolver,
    InstructionContract, ProtocolLock,
};

mod accounts;
mod bidder;
mod discovery;
mod execute;
mod settler;
mod signer;
mod transaction;

use discovery::{observe, Observation, RpcClient};
use bidder::{BidPolicy, BidderJob};
use settler::{SettlePolicy, SettlerJob};
use execute::TriggerJob;
use keeper_core::OutcomeStatus;
use signer::{LocalKeypair, TransactionSigner};

/// What the last discovery pass saw, shared with the health endpoint.
#[derive(Clone, Debug, Default)]
struct Snapshot {
    observation: Observation,
    error: Option<String>,
    passes: u64,
    /// Execution counters. Sent is deliberately separate from evaluated: on a
    /// healthy market the first climbs and the second does not, and an
    /// operator needs to see that the keeper is working rather than idle.
    evaluated: u64,
    sent: u64,
    last_signature: Option<String>,
    last_execution_error: Option<String>,
    /// Why the last pass declined to act, counted by reason.
    ///
    /// Without this, "every position is healthy" and "every simulation is
    /// failing because the keeper builds a malformed transaction" both read as
    /// zero sends. They are the two most important states to tell apart.
    last_reasons: BTreeMap<String, u32>,
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
    instruction_contract: PathBuf,
    account_layout: PathBuf,
    account_resolution: PathBuf,
    /// Ceiling on transactions per pass. A keeper that has gone wrong should
    /// run out of permission before it runs out of money.
    max_sends_per_pass: usize,
    /// Below this the keeper reports unready rather than discovering it
    /// cannot pay halfway through a liquidation.
    minimum_lamports: u64,
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
        let instruction_contract = env::var("DUSK_INSTRUCTION_CONTRACT")
            .unwrap_or_else(|_| "protocol/keeper-instructions.v1.json".into())
            .into();
        let account_layout = env::var("DUSK_ACCOUNT_LAYOUT")
            .unwrap_or_else(|_| "protocol/keeper-account-layout.v1.json".into())
            .into();
        let account_resolution = env::var("DUSK_ACCOUNT_RESOLUTION")
            .unwrap_or_else(|_| "protocol/keeper-account-resolution.v1.json".into())
            .into();
        let max_sends_per_pass = env::var("KEEPER_MAX_SENDS_PER_PASS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(4);
        let minimum_lamports = env::var("KEEPER_MINIMUM_LAMPORTS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(20_000_000);
        Ok(Self {
            account_layout,
            account_resolution,
            bind_address,
            instruction_contract,
            max_sends_per_pass,
            minimum_lamports,
            mode,
            profile,
            protocol_lock,
        })
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::from_environment()?;
    let lock = ProtocolLock::read(&config.protocol_lock)?;

    if config.mode == Mode::Live {
        lock.assert_live_ready()?;
    }

    println!(
        "keeper profile={} mode={} protocol_revision={}",
        config.profile,
        if config.mode == Mode::Live { "live" } else { "shadow" },
        lock.revision
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

    // A profile that signs needs a key; one that does not must never have
    // one. The sentinel is the whole reason this is a per-profile decision
    // rather than a global switch.
    let executor: Option<LocalKeypair> = if config.profile == "sentinel" {
        None
    } else {
        match load_signer() {
            Ok(keypair) => {
                println!("keeper signer={}", keypair.public_key_base58());
                Some(keypair)
            }
            // Absent in shadow mode is expected; absent in live mode is not,
            // and refusing to start is better than running as a keeper that
            // silently never acts.
            Err(error) if config.mode == Mode::Shadow => {
                println!("keeper signer=none ({error})");
                None
            }
            Err(error) => return Err(Box::new(error)),
        }
    };

    let contract = match std::fs::read_to_string(&config.instruction_contract) {
        Ok(raw) => Some(InstructionContract::from_json(&raw)?),
        Err(error) if executor.is_none() => {
            println!("keeper contract=absent ({error})");
            None
        }
        Err(error) => return Err(Box::new(error)),
    };

    let resolver = match std::fs::read_to_string(&config.account_resolution) {
        Ok(raw) => {
            let manifest = AccountResolutionManifest::from_json(&raw)?;
            contract
                .as_ref()
                .map(|contract| DeterministicAccountResolver::new(&lock, contract, manifest))
                .transpose()?
        }
        Err(error) if executor.is_none() => {
            println!("keeper resolution=absent ({error})");
            None
        }
        Err(error) => return Err(Box::new(error)),
    };

    // Offsets generated for a different deployment are not approximately
    // right, they are arbitrary, so a mismatch stops the keeper rather than
    // letting it read whatever happens to sit at those bytes.
    let layout = match std::fs::read_to_string(&config.account_layout) {
        Ok(raw) => {
            let manifest = AccountLayoutManifest::from_json(&raw)?;
            manifest.assert_matches(&lock.revision)?;
            Some(manifest)
        }
        Err(error) if executor.is_none() => {
            println!("keeper layout=absent ({error})");
            None
        }
        Err(error) => return Err(Box::new(error)),
    };

    // Discovery runs even in shadow mode: reading is how the sentinel earns
    // trust before any profile is given a key.
    if let Ok(endpoint) = env::var("SOLANA_RPC_HTTP_URL") {
        let interval = env::var("DISCOVERY_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(15_000);
        let shared = Arc::clone(&snapshot);
        let program_id_key = decode_program_id(&dusk_program_id)?;
        let dry_run = config.mode == Mode::Shadow;
        let profile = config.profile.clone();
        let max_sends = config.max_sends_per_pass;
        let minimum_lamports = config.minimum_lamports;
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

                if let (Some(signer), Some(contract), Some(layout), Some(resolver)) = (
                    executor.as_ref(),
                    contract.as_ref(),
                    layout.as_ref(),
                    resolver.as_ref(),
                ) {
                    run_execution_pass(
                        &client,
                        contract,
                        layout,
                        resolver,
                        signer,
                        &profile,
                        program_id_key,
                        max_sends,
                        minimum_lamports,
                        dry_run,
                        &shared,
                    );
                }

                thread::sleep(Duration::from_millis(interval));
            }
        });
    }

    serve(config.bind_address, &config.profile, &lock.revision, snapshot)
}


/// The signer, from the environment or from a file.
///
/// The variable is how a deployed keeper receives a key, since there is no
/// filesystem to mount one onto; the path is how a local run uses an existing
/// Solana keypair without pasting it into a shell.
fn load_signer() -> Result<LocalKeypair, signer::SignerError> {
    match env::var("KEEPER_SIGNER_KEY_PATH") {
        Ok(path) => LocalKeypair::from_file(path),
        Err(_) => LocalKeypair::from_environment("KEEPER_SIGNER_KEY"),
    }
}

fn decode_program_id(encoded: &str) -> Result<[u8; 32], Box<dyn Error>> {
    let bytes = bs58::decode(encoded).into_vec()?;
    if bytes.len() != 32 {
        return Err(format!("program id is {} bytes, not 32", bytes.len()).into());
    }
    let mut key = [0_u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Fill any auction the trigger profile has opened.
///
/// Candidate discovery is shared with the trigger — the same positions, read
/// the same way — but the two profiles want opposite halves of it: the trigger
/// wants positions with no auction running, the bidder wants the ones that
/// have one.
fn bid_pass(
    trigger: &TriggerJob<'_>,
    contract: &InstructionContract,
    layout: &AccountLayoutManifest,
    resolver: &DeterministicAccountResolver,
    signer: &LocalKeypair,
    dry_run: bool,
) -> Result<Vec<execute::AttemptReport>, execute::ExecutionError> {
    let Some(policy) = bid_policy() else {
        // A bidder with no declared price is not a conservative bidder, it is
        // one that will accept anything. It declines to run instead.
        return Ok(Vec::new());
    };
    let bidder = BidderJob {
        client: trigger.client,
        contract,
        dry_run,
        layout,
        policy,
        resolver,
        signer,
    };
    let mut reports = Vec::new();
    for position in bidder.candidates(trigger.all_positions()?) {
        reports.push(bidder.attempt(&position)?);
    }
    Ok(reports)
}

/// The bidder's economic bounds, all required.
///
/// There is no default that is right for every market: these encode what the
/// operator believes the collateral is worth, which the keeper has no
/// independent source for. Guessing on the operator's behalf would mean
/// guessing with the operator's money.
fn bid_policy() -> Option<BidPolicy> {
    Some(BidPolicy {
        max_repay: env::var("KEEPER_BID_MAX_REPAY").ok()?.parse().ok()?,
        min_collateral_bps: env::var("KEEPER_BID_MIN_COLLATERAL_BPS")
            .ok()?
            .parse()
            .ok()?,
        slippage_bps: env::var("KEEPER_BID_SLIPPAGE_BPS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(50),
    })
}

/// Settle any auction that has run its course.
fn settle_pass(
    trigger: &TriggerJob<'_>,
    contract: &InstructionContract,
    layout: &AccountLayoutManifest,
    resolver: &DeterministicAccountResolver,
    signer: &LocalKeypair,
    dry_run: bool,
) -> Result<Vec<execute::AttemptReport>, execute::ExecutionError> {
    let Some(policy) = settle_policy() else {
        return Ok(Vec::new());
    };
    let settler = SettlerJob {
        client: trigger.client,
        contract,
        dry_run,
        layout,
        policy,
        resolver,
        signer,
    };
    let mut reports = Vec::new();
    for position in settler.candidates(trigger.all_positions()?) {
        reports.push(settler.attempt(&position)?);
    }
    Ok(reports)
}

/// What the settler will accept for calling the backstop.
///
/// Required rather than defaulted, for the same reason the bidder's bounds
/// are: the floor encodes what the operator thinks the work is worth, and a
/// settler that accepts anything will settle at a loss.
fn settle_policy() -> Option<SettlePolicy> {
    Some(SettlePolicy {
        min_bounty: env::var("KEEPER_SETTLE_MIN_BOUNTY").ok()?.parse().ok()?,
        slippage_bps: env::var("KEEPER_SETTLE_SLIPPAGE_BPS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(50),
    })
}

/// One execution pass, folded into the shared snapshot.
///
/// The balance floor is checked here rather than at startup because a keeper
/// that has been running for a week is exactly the one that has spent its
/// SOL, and discovering that mid-liquidation is how a position ends up
/// half-handled.
#[allow(clippy::too_many_arguments)]
fn run_execution_pass(
    client: &RpcClient,
    contract: &InstructionContract,
    layout: &AccountLayoutManifest,
    resolver: &DeterministicAccountResolver,
    signer: &LocalKeypair,
    profile: &str,
    program_id: [u8; 32],
    max_sends_per_pass: usize,
    minimum_lamports: u64,
    dry_run: bool,
    shared: &Arc<Mutex<Snapshot>>,
) {
    if !dry_run {
        match client.lamports(&signer.public_key_base58()) {
            Ok(balance) if balance < minimum_lamports => {
                if let Ok(mut current) = shared.lock() {
                    current.last_execution_error = Some(format!(
                        "signer holds {balance} lamports, below the {minimum_lamports} floor"
                    ));
                }
                return;
            }
            Ok(_) => {}
            Err(error) => {
                if let Ok(mut current) = shared.lock() {
                    current.last_execution_error = Some(error.to_string());
                }
                return;
            }
        }
    }

    let job = TriggerJob {
        client,
        confirmation_attempts: 8,
        confirmation_interval: Duration::from_millis(1_500),
        contract,
        dry_run,
        layout,
        max_sends_per_pass,
        program_id,
        signer,
    };

    // Which job a service runs is its profile, not a flag: one service, one
    // wallet, one thing it is allowed to do. A profile with no job here does
    // nothing rather than falling back to something it was not deployed for.
    let outcome = match profile {
        "lending-bidder" => bid_pass(&job, contract, layout, resolver, signer, dry_run),
        "lending-settler" => settle_pass(&job, contract, layout, resolver, signer, dry_run),
        "lending-trigger" => job.run_pass(),
        _ => Ok(Vec::new()),
    };

    match outcome {
        Ok(reports) => {
            if let Ok(mut current) = shared.lock() {
                current.evaluated += reports.len() as u64;
                current.last_execution_error = None;
                current.last_reasons = reports.iter().fold(
                    BTreeMap::new(),
                    |mut counts, report| {
                        *counts
                            .entry(
                                serde_json::to_value(report.reason)
                                    .ok()
                                    .and_then(|value| value.as_str().map(str::to_owned))
                                    .unwrap_or_else(|| "unknown".into()),
                            )
                            .or_insert(0) += 1;
                        counts
                    },
                );
                for report in &reports {
                    if report.status == OutcomeStatus::Executed {
                        current.sent += 1;
                        current.last_signature = report.signature.clone();
                        println!(
                            "keeper executed position={} debt_mint={} signature={}",
                            report.position,
                            report.debt_mint,
                            report.signature.as_deref().unwrap_or("-")
                        );
                    } else if report.reason == keeper_core::ReasonCode::SimulationRejected {
                        // An unrecognized rejection is the one skip worth
                        // printing: the expected ones are silent by design,
                        // so anything reaching here is either a new program
                        // error or a keeper that is building bad transactions.
                        println!(
                            "keeper unclassified position={} detail={}",
                            report.position,
                            report.detail.as_deref().unwrap_or("-")
                        );
                    } else if report.status == OutcomeStatus::RetryableFailure {
                        println!(
                            "keeper submit_failed position={} detail={}",
                            report.position,
                            report.detail.as_deref().unwrap_or("-")
                        );
                    }
                }
            }
        }
        Err(error) => {
            if let Ok(mut current) = shared.lock() {
                current.last_execution_error = Some(error.to_string());
            }
        }
    }
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
    let reasons = serde_json::to_string(&snapshot.last_reasons)
        .unwrap_or_else(|_| "{}".to_owned());
    let observed = format!("{observed},\"lastReasons\":{reasons}");
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
