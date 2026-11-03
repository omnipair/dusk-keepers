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
}

impl Config {
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
            lending_mode,
            protocol_lock,
        })
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
