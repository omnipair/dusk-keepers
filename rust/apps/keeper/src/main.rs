//! Keeper startup, polling loop and execution reporting.
mod config;
mod health;
mod liquidation;
mod runtime;
mod workers;

use crate::{
    liquidation::policy::LiquidationMode,
    runtime::rpc::RpcClient,
    runtime::signer::{LocalKeypair, TransactionSigner},
    workers::{
        leverage::orders,
        sentinel::{fee_notice, observe},
    },
};
use config::{Config, Mode};
use dusk_adapter::{
    AccountLayoutManifest, AccountResolutionManifest, DeterministicAccountResolver,
    InstructionContract, ProtocolLock,
};
use health::{Snapshot, serve};
use keeper_core::OutcomeStatus;
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().any(|a| a == "--order-plan") {
        return orders::run_plan_cli();
    }
    if env::args().any(|a| a == "--flash-plan") {
        return liquidation::cli::run();
    }
    let config = Config::from_environment()?;
    let lock = ProtocolLock::read(&config.protocol_lock)?;

    if config.mode == Mode::Live {
        if runtime::devnet::enabled() {
            if config.max_sends_per_pass != 1 {
                return Err("devnet testing requires KEEPER_MAX_SENDS_PER_PASS=1".into());
            }
            let endpoint = env::var("SOLANA_RPC_HTTP_URL")?;
            runtime::devnet::verify(&RpcClient::new(endpoint, Duration::from_secs(20)), true)?;
            println!(
                "verified explicit devnet test exception; frozen release gate remains required on other networks"
            );
        } else {
            lock.assert_live_ready()?;
        }
    }

    println!(
        "keeper profile={} mode={} protocol_revision={}",
        config.profile,
        if config.mode == Mode::Live {
            "live"
        } else {
            "shadow"
        },
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
    let executor: Option<Box<dyn TransactionSigner>> = if config.profile == "sentinel" {
        None
    } else if config.mode == Mode::Shadow {
        let public = env::var("KEEPER_SHADOW_PUBLIC_KEY")
            .ok()
            .and_then(|p| crate::runtime::accounts::decode_key(&p));
        public.map(|key| {
            Box::new(crate::runtime::signer::ShadowIdentity(key)) as Box<dyn TransactionSigner>
        })
    } else {
        Some(Box::new(load_signer()?))
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

    if env::args().any(|arg| arg == "--prepare-devnet-wallet") {
        if config.mode != Mode::Live {
            return Err("wallet preparation requires live devnet mode".into());
        }
        let client = RpcClient::new(env::var("SOLANA_RPC_HTTP_URL")?, Duration::from_secs(20));
        let priority_markets = std::collections::BTreeSet::new();
        let context = workers::WorkerContext {
            client: &client,
            contract: contract.as_ref().ok_or("instruction contract required")?,
            layout: layout.as_ref().ok_or("layout required")?,
            resolver: resolver.as_ref().ok_or("resolver required")?,
            signer: executor.as_deref().ok_or("wallet required")?,
            program_id: decode_program_id(&dusk_program_id)?,
            dry_run: false,
            max_sends: 1,
            priority_markets: &priority_markets,
        };
        let signature =
            runtime::devnet::prepare_wallet(&client, &context, config.minimum_lamports)?;
        println!(
            "devnet wallet preparation finalized_signature={signature:?}; check journal for pending submissions or existing accounts"
        );
        return Ok(());
    }

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
        let lending_mode = config.lending_mode;
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

                if let Some(layout) = layout.as_ref() {
                    match fee_notice::monitor(&client, &dusk_program_id, layout) {
                        Ok(notices) => {
                            if !notices.final_notice_mints.is_empty() {
                                println!(
                                    "fee_notice_final epoch={} mints={} action=recheck_all_positions",
                                    notices.epoch,
                                    notices.final_notice_mints.len()
                                );
                            }
                            if let Ok(mut current) = shared.lock() {
                                current.fee_notices = Some(notices);
                                current.fee_notice_error = None;
                            }
                        }
                        Err(error) => {
                            if let Ok(mut current) = shared.lock() {
                                current.fee_notice_error = Some(error.to_string());
                            }
                        }
                    }
                } else {
                    if let Ok(mut current) = shared.lock() {
                        current.fee_notice_error =
                            Some("fee notice monitor needs the pinned account layout".into());
                    }
                }

                if let (Some(signer), Some(contract), Some(layout), Some(resolver)) = (
                    executor.as_ref(),
                    contract.as_ref(),
                    layout.as_ref(),
                    resolver.as_ref(),
                ) {
                    if let Ok(mut current) = shared.lock() {
                        current.execution_configured = true;
                        current.execution_passes += 1;
                    }
                    run_execution_pass(
                        &client,
                        contract,
                        layout,
                        resolver,
                        signer.as_ref(),
                        &profile,
                        lending_mode,
                        program_id_key,
                        max_sends,
                        minimum_lamports,
                        dry_run,
                        &shared,
                    );
                }

                if env::args().any(|a| a == "--once") {
                    let current = shared.lock().expect("snapshot lock");
                    println!(
                        "keeper_once evaluated={} detail={:?} error={:?}",
                        current.evaluated, current.last_skip_detail, current.last_execution_error
                    );
                    std::process::exit(
                        if current.error.is_some()
                            || current.fee_notice_error.is_some()
                            || current.last_execution_error.is_some()
                        {
                            1
                        } else {
                            0
                        },
                    );
                }
                thread::sleep(Duration::from_millis(interval));
            }
        });
    }

    serve(
        config.bind_address,
        &config.profile,
        &lock.revision,
        snapshot,
    )
}

/// The signer, from the environment or from a file.
///
/// The variable is how a deployed keeper receives a key, since there is no
/// filesystem to mount one onto; the path is how a local run uses an existing
/// Solana keypair without pasting it into a shell.
fn load_signer() -> Result<LocalKeypair, crate::runtime::signer::SignerError> {
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
    signer: &dyn TransactionSigner,
    profile: &str,
    lending_mode: LiquidationMode,
    program_id: [u8; 32],
    max_sends_per_pass: usize,
    minimum_lamports: u64,
    dry_run: bool,
    shared: &Arc<Mutex<Snapshot>>,
) {
    if !dry_run {
        if runtime::devnet::enabled()
            && let Err(error) = runtime::devnet::verify(client, false)
        {
            if let Ok(mut current) = shared.lock() {
                current.last_execution_error = Some(error);
            }
            return;
        }
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

    // Priority is discovery data only; every attempted action still simulates
    // and revalidates the current mint/Clock snapshot before re-signing.
    let priority_markets: std::collections::BTreeSet<_> = shared
        .lock()
        .ok()
        .and_then(|snapshot| {
            snapshot
                .fee_notice_error
                .is_none()
                .then(|| snapshot.fee_notices.clone())
                .flatten()
        })
        .map(|notice| {
            notice
                .final_notice_markets
                .into_iter()
                .filter_map(|key| crate::runtime::accounts::decode_key(&key))
                .collect()
        })
        .unwrap_or_default();
    let outcome = workers::run(
        profile,
        lending_mode,
        &workers::WorkerContext {
            client,
            contract,
            layout,
            resolver,
            signer,
            program_id,
            dry_run,
            max_sends: max_sends_per_pass,
            priority_markets: &priority_markets,
        },
    );

    match outcome {
        Ok(reports) => {
            if let Ok(mut current) = shared.lock() {
                current.evaluated += reports.len() as u64;
                current.last_execution_error = None;
                current.last_skip_detail = reports
                    .iter()
                    .find(|report| report.status != OutcomeStatus::Executed)
                    .and_then(|report| report.detail.clone());
                current.last_reasons =
                    reports.iter().fold(BTreeMap::new(), |mut counts, report| {
                        *counts
                            .entry(
                                serde_json::to_value(report.reason)
                                    .ok()
                                    .and_then(|value| value.as_str().map(str::to_owned))
                                    .unwrap_or_else(|| "unknown".into()),
                            )
                            .or_insert(0) += 1;
                        counts
                    });
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
