//! Validated service profile and runtime configuration.
use crate::liquidation::policy::LiquidationMode;
use std::{env, error::Error, net::SocketAddr, path::PathBuf};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode {
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
pub(crate) struct Config {
    pub mode: Mode,
    pub profile: String,
    pub lending_mode: LiquidationMode,
    pub bind_address: SocketAddr,
    pub protocol_lock: PathBuf,
    pub instruction_contract: PathBuf,
    pub account_layout: PathBuf,
    pub account_resolution: PathBuf,
    /// Ceiling on transactions per pass. A keeper that has gone wrong should
    /// run out of permission before it runs out of money.
    pub max_sends_per_pass: usize,
    /// Below this the keeper reports unready rather than discovering it
    /// cannot pay halfway through a liquidation.
    pub minimum_lamports: u64,
    pub discovery_interval_ms: u64,
    pub max_scan_age_ms: u64,
}

impl Config {
    /// Missing spending settings must fail at startup, even on an empty market.
    pub fn validate_mainnet_execution(&self) -> Result<(), String> {
        use crate::liquidation::required_u64;
        match self.profile.as_str() {
            "lending" | "leverage" => {
                if self.profile == "leverage" || self.lending_mode != LiquidationMode::Emergency {
                    required_u64("KEEPER_FLASH_MAX_REPAY")?;
                    match env::var("KEEPER_FLASH_SETTLEMENT")
                        .as_deref()
                        .unwrap_or("jupiter")
                    {
                        "inventory" => {
                            required_u64("KEEPER_FLASH_MAX_PAYMENT_DEBIT")?;
                            required_u64("KEEPER_FLASH_MIN_COLLATERAL_CREDIT")?;
                        }
                        "jupiter" => {
                            crate::routing::jupiter::Config::from_env()?;
                        }
                        "dusk" => {}
                        _ => return Err("invalid KEEPER_FLASH_SETTLEMENT".into()),
                    }
                }
                required_u64("KEEPER_LIQUIDATION_MIN_PROFIT")?;
                if number("KEEPER_EMERGENCY_SLIPPAGE_BPS", 0)? > 10_000 {
                    return Err("KEEPER_EMERGENCY_SLIPPAGE_BPS must be at most 10000".into());
                }
                number("KEEPER_ORDER_MIN_BOUNTY_CREDIT", 0)?;
            }
            "hlp-recovery" => {
                crate::liquidation::plan::key(
                    &env::var("KEEPER_HLP_MARKET").map_err(|_| "KEEPER_HLP_MARKET is required")?,
                )?;
                for name in [
                    "KEEPER_HLP_MAX_SOCIALIZED_LOSS",
                    "KEEPER_HLP_MAX_PAYMENT_DEBIT",
                    "KEEPER_HLP_MIN_COLLATERAL_CREDIT",
                    "KEEPER_HLP_MIN_BOUNTY_OUT",
                ] {
                    required_u64(name)?;
                }
            }
            "auction-arbitrage" => {
                required_u64("KEEPER_AUCTION_MAX_SOLD")?;
                required_u64("KEEPER_AUCTION_MAX_PAYMENT")?;
                if number("KEEPER_AUCTION_SLIPPAGE_BPS", 50)? > 10_000 {
                    return Err("KEEPER_AUCTION_SLIPPAGE_BPS must be at most 10000".into());
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn from_environment() -> Result<Self, Box<dyn Error>> {
        let mode = Mode::parse(&env::var("KEEPER_MODE").unwrap_or_else(|_| "shadow".into()))?;
        let profile = env::var("KEEPER_PROFILE").unwrap_or_else(|_| "conformance".into());
        if !matches!(
            profile.as_str(),
            "conformance"
                | "lending"
                | "leverage"
                | "hlp-recovery"
                | "auction-arbitrage"
                | "lifecycle"
                | "sentinel"
        ) {
            return Err(format!("unsupported KEEPER_PROFILE: {profile}").into());
        }
        let lending_mode = if profile == "lending" {
            LiquidationMode::parse(
                &env::var("KEEPER_LENDING_MODE").unwrap_or_else(|_| "both".into()),
            )?
        } else {
            LiquidationMode::Both
        };
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
        let max_sends_per_pass = usize::try_from(number("KEEPER_MAX_SENDS_PER_PASS", 4)?)?;
        let minimum_lamports = number("KEEPER_MINIMUM_LAMPORTS", 20_000_000)?;
        let discovery_interval_ms = number("DISCOVERY_INTERVAL_MS", 5_000)?;
        let max_scan_age_ms = number("KEEPER_MAX_SCAN_AGE_MS", 120_000)?;
        if discovery_interval_ms == 0 || max_scan_age_ms == 0 {
            return Err("discovery interval and maximum scan age must be positive".into());
        }
        Ok(Self {
            account_layout,
            account_resolution,
            bind_address,
            instruction_contract,
            max_sends_per_pass,
            minimum_lamports,
            discovery_interval_ms,
            max_scan_age_ms,
            mode,
            profile,
            lending_mode,
            protocol_lock,
        })
    }
}

fn number(name: &str, default: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(value) => value.parse().map_err(|_| format!("invalid {name}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("invalid {name}")),
    }
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
