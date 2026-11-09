//! Typed Swap API V2 client. The keeper owns composition, signing and submission.
#[cfg(test)]
mod tests;
mod validation;

use crate::liquidation::plan::address;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    sync::Mutex,
    time::{Duration, Instant},
};
pub use validation::{Request, ValidatedRoute};

type Result<T> = std::result::Result<T, String>;
pub const PROGRAM: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
const ENDPOINT: &str = "https://api.jup.ag/swap/v2/build";
const MAX_BODY: u64 = 1_048_576;
// Shared across lending/leverage clients in this process, including retries.
static NEXT_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

pub struct Config {
    pub slippage_bps: u16,
    pub max_accounts: u8,
    pub attempts: usize,
    pub max_age: Duration,
    pub priority_lamports: u64,
    pub max_sol_debit: u64,
    timeout: Duration,
    interval: Duration,
    api_key: Option<String>,
}
fn setting(name: &str, default: u64, low: u64, high: u64) -> Result<u64> {
    let value = std::env::var(name).map_or(Ok(default), |s| {
        s.parse::<u64>().map_err(|_| format!("invalid {name}"))
    })?;
    if !(low..=high).contains(&value) {
        return Err(format!("{name} must be {low}..{high}"));
    }
    Ok(value)
}
impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            slippage_bps: setting("KEEPER_JUPITER_SLIPPAGE_BPS", 50, 0, 500)? as u16,
            max_accounts: setting("KEEPER_JUPITER_MAX_ACCOUNTS", 32, 8, 64)? as u8,
            attempts: setting("KEEPER_JUPITER_MAX_BUILDS", 4, 1, 16)? as usize,
            max_age: Duration::from_millis(setting(
                "KEEPER_JUPITER_MAX_AGE_MS",
                10_000,
                1000,
                30_000,
            )?),
            priority_lamports: setting("KEEPER_JUPITER_PRIORITY_FEE_LAMPORTS", 0, 0, 100_000)?,
            max_sol_debit: setting(
                "KEEPER_JUPITER_MAX_SOL_DEBIT_LAMPORTS",
                20_000_000,
                5000,
                100_000_000,
            )?,
            timeout: Duration::from_millis(setting(
                "KEEPER_JUPITER_TIMEOUT_MS",
                5000,
                100,
                10_000,
            )?),
            interval: Duration::from_millis(setting(
                "KEEPER_JUPITER_REQUEST_INTERVAL_MS",
                2100,
                100,
                10_000,
            )?),
            api_key: std::env::var("JUPITER_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
        })
    }
    pub fn pass_budget() -> Result<usize> {
        Ok(setting("KEEPER_JUPITER_MAX_BUILDS_PER_PASS", 12, 1, 64)? as usize)
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiMeta {
    pubkey: String,
    is_signer: bool,
    is_writable: bool,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiInstruction {
    program_id: String,
    accounts: Vec<ApiMeta>,
    data: String,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildResponse {
    input_mint: String,
    output_mint: String,
    in_amount: String,
    out_amount: String,
    other_amount_threshold: String,
    swap_mode: String,
    slippage_bps: u16,
    transaction_version: u8,
    setup_instructions: Vec<ApiInstruction>,
    swap_instruction: ApiInstruction,
    compute_budget_instructions: Vec<ApiInstruction>,
    cleanup_instruction: Option<ApiInstruction>,
    other_instructions: Vec<ApiInstruction>,
    tip_instruction: Option<ApiInstruction>,
    addresses_by_lookup_table_address: Option<BTreeMap<String, Vec<String>>>,
}

pub struct Client {
    agent: ureq::Agent,
    endpoint: String,
}
impl Client {
    pub fn new(config: &Config) -> Self {
        Self {
            endpoint: ENDPOINT.into(),
            agent: ureq::AgentBuilder::new()
                .timeout(config.timeout)
                .redirects(0)
                .build(),
        }
    }
    pub fn build(
        &self,
        config: &Config,
        request: &Request,
        max_accounts: u8,
        pass_budget: &mut usize,
    ) -> Result<Option<ValidatedRoute>> {
        if *pass_budget == 0 {
            return Err("Jupiter pass request budget exhausted; retry next pass".into());
        }
        let mut next = NEXT_REQUEST
            .lock()
            .map_err(|_| "Jupiter rate limiter unavailable")?;
        if let Some(time) = *next {
            let delay = time.saturating_duration_since(Instant::now());
            // Backoff never blocks an entire worker pass.
            if delay > config.interval {
                return Err("Jupiter temporarily backed off; retry later".into());
            }
            std::thread::sleep(delay);
        }
        *next = Some(Instant::now() + config.interval);
        drop(next);
        *pass_budget -= 1;
        let built_at = Instant::now();
        let mut call = self
            .agent
            .get(&self.endpoint)
            .query("inputMint", &address(request.input_mint))
            .query("outputMint", &address(request.output_mint))
            .query("amount", &request.amount.to_string())
            .query("taker", &address(request.buyer))
            .query("destinationTokenAccount", &address(request.destination))
            .query("slippageBps", &config.slippage_bps.to_string())
            .query("maxAccounts", &max_accounts.to_string())
            .query("wrapAndUnwrapSol", "false")
            .query("transactionVersion", "0");
        if let Some(key) = &config.api_key {
            call = call.set("x-api-key", key);
        }
        let response = match call.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(status, r)) => {
                if status == 429 || status >= 500 {
                    *NEXT_REQUEST
                        .lock()
                        .map_err(|_| "Jupiter rate limiter unavailable")? =
                        Some(Instant::now() + Duration::from_secs(30));
                }
                let body = read_body(r)?;
                return classify_error(status, &body);
            }
            // Do not expose API credentials, response bodies or transport URLs.
            Err(_) => return Err("Jupiter request failed or timed out; retry later".into()),
        };
        let body = read_body(response)?;
        let response: BuildResponse =
            serde_json::from_slice(&body).map_err(|_| "malformed Jupiter build response")?;
        let mut route = validation::validate(response, request, config.slippage_bps)?;
        route.expires_at = built_at + config.max_age;
        Ok(Some(route))
    }
}
fn read_body(response: ureq::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|_| "Jupiter response read failed")?;
    if body.len() as u64 > MAX_BODY {
        return Err("Jupiter response exceeds size limit".into());
    }
    Ok(body)
}
fn classify_error<T>(status: u16, body: &[u8]) -> Result<Option<T>> {
    let value: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    if status == 400 && value["error"] == "No routes found" {
        return Ok(None);
    }
    Err(format!("Jupiter HTTP {status}; route evaluation aborted"))
}
