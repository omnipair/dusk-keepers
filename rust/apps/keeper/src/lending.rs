//! Lending execution policy. Select a route before entering durable submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiquidationMode {
    Ordinary,
    Emergency,
    Both,
}

impl LiquidationMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ordinary" => Ok(Self::Ordinary),
            "emergency" => Ok(Self::Emergency),
            "both" => Ok(Self::Both),
            _ => Err(format!(
                "KEEPER_LENDING_MODE must be ordinary, emergency, or both, got {value}"
            )),
        }
    }
}

/// Only an explicitly unavailable route permits fallback. Errors abort selection;
/// callers submit the selected plan once, outside this function. In particular,
/// an unknown submitted signature must never be interpreted as an absent route.
pub fn select_liquidation<T>(
    mode: LiquidationMode,
    ordinary: impl FnOnce() -> Result<Option<T>, String>,
    emergency: impl FnOnce() -> Result<Option<T>, String>,
) -> Result<Option<T>, String> {
    if mode != LiquidationMode::Emergency
        && let Some(plan) = ordinary()?
    {
        return Ok(Some(plan));
    }
    if mode != LiquidationMode::Ordinary {
        return emergency();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_success_never_evaluates_emergency() {
        assert_eq!(
            select_liquidation(
                LiquidationMode::Both,
                || Ok(Some("flash")),
                || panic!("emergency evaluated after ordinary success")
            )
            .unwrap(),
            Some("flash")
        );
    }

    #[test]
    fn only_an_unavailable_ordinary_route_permits_fallback() {
        assert_eq!(
            select_liquidation(LiquidationMode::Both, || Ok(None), || Ok(Some("emergency")))
                .unwrap(),
            Some("emergency")
        );
        assert_eq!(
            select_liquidation::<()>(
                LiquidationMode::Both,
                || Err("RPC or invariant failure".into()),
                || panic!("error triggered fallback")
            )
            .unwrap_err(),
            "RPC or invariant failure"
        );
    }

    #[test]
    fn restricted_modes_never_evaluate_the_other_route() {
        assert_eq!(
            select_liquidation::<()>(
                LiquidationMode::Ordinary,
                || Ok(None),
                || panic!("ordinary-only fallback")
            )
            .unwrap(),
            None
        );
        assert_eq!(
            select_liquidation(
                LiquidationMode::Emergency,
                || panic!("emergency-only ordinary probe"),
                || Ok(Some("emergency"))
            )
            .unwrap(),
            Some("emergency")
        );
    }

    #[test]
    fn invalid_modes_fail_closed() {
        assert_eq!(
            LiquidationMode::parse("both").unwrap(),
            LiquidationMode::Both
        );
        assert!(LiquidationMode::parse("bidder").is_err());
    }
}
