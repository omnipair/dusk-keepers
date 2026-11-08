//! Snapshot fingerprints, simulation post-state and expected rejection checks.
use super::plan::Snapshot;
use dusk_adapter::transfer_fees::CLOCK;
use sha2::{Digest, Sha256};
type Result<T> = std::result::Result<T, String>;
pub(crate) fn required_u64(name: &str) -> Result<u64> {
    std::env::var(name)
        .map_err(|_| format!("{name} must be configured in debt-token atoms"))?
        .parse()
        .map_err(|_| format!("invalid {name}"))
}
pub(crate) fn post_bytes(value: &serde_json::Value, index: usize) -> Result<Option<Vec<u8>>> {
    let a = value["value"]["accounts"]
        .as_array()
        .and_then(|v| v.get(index))
        .ok_or("simulation omitted requested post-state")?;
    if a.is_null() {
        return Ok(None);
    }
    if a["data"][1] != "base64" {
        return Err("post-state encoding".into());
    }
    // RPC simulation can return a reclaimed System account rather than null.
    // Only the complete closed-account shape is equivalent to absence.
    if a["lamports"].as_u64() == Some(0) {
        if a["owner"] == "11111111111111111111111111111111"
            && a["executable"] == false
            && a["data"][0] == ""
        {
            return Ok(None);
        }
        return Err("invalid closed post-state".into());
    }
    Ok(Some(
        crate::runtime::rpc::decode_base64(a["data"][0].as_str().ok_or("post-state data")?)
            .ok_or("post-state base64")?,
    ))
}

#[cfg(test)]
mod post_state_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn accepts_null_and_reclaimed_system_account_but_not_invalid_closures() {
        let closed = json!({"owner":"11111111111111111111111111111111", "executable":false, "lamports":0,"data":["","base64"]});
        for account in [serde_json::Value::Null, closed.clone()] {
            assert_eq!(
                post_bytes(&json!({"value":{"accounts":[account]}}), 0).unwrap(),
                None
            );
        }
        for (field, value) in [
            ("owner", json!("wrong")),
            ("executable", json!(true)),
            ("data", json!(["AQ==", "base64"])),
        ] {
            let mut account = closed.clone();
            account[field] = value;
            assert!(post_bytes(&json!({"value":{"accounts":[account]}}), 0).is_err());
        }
        let mut funded = closed;
        funded["lamports"] = 1.into();
        assert_eq!(
            post_bytes(&json!({"value":{"accounts":[funded]}}), 0).unwrap(),
            Some(vec![])
        );
        assert!(post_bytes(&json!({"value":{"accounts":[]}}), 0).is_err());
    }
}
pub(crate) fn state_hash(s: &Snapshot, epoch: u64) -> Result<String> {
    let mut accounts = s.accounts.clone();
    accounts.remove(CLOCK);
    Ok(format!(
        "{:x}",
        Sha256::digest(
            [
                serde_json::to_vec(&accounts).map_err(|e| e.to_string())?,
                epoch.to_le_bytes().to_vec()
            ]
            .concat()
        )
    ))
}

// Financial bounds are expected during size probing. ABI, account, invariant,
// and arithmetic failures must remain visible instead of looking unprofitable.
pub(super) fn expected_bound_rejection(value: &serde_json::Value) -> bool {
    let Some(logs) = value["value"]["logs"].as_array() else {
        return false;
    };
    [
        "PositionNotLiquidatable",
        "LiquidationRepayTooLarge",
        "LiquidationDoesNotImproveHealth",
        "InsufficientAmount",
        "InsufficientDebt",
        "SlippageExceeded",
        "InsufficientCashReserve0",
        "InsufficientCashReserve1",
    ]
    .iter()
    .any(|name| {
        logs.iter().any(|l| {
            l.as_str()
                .is_some_and(|line| line.contains(&format!("Error Code: {name}.")))
        })
    })
}

pub(crate) fn compute_limit_rejection(value: &serde_json::Value) -> bool {
    // RPCs may expose the structured error without the compact enum spelling
    // in logs. A compute-bound large fill must not stop smaller probes.
    if value["value"]["err"]
        .to_string()
        .contains("ComputationalBudgetExceeded")
    {
        return true;
    }
    value["value"]["logs"].as_array().is_some_and(|logs| {
        logs.iter().any(|v| {
            v.as_str().is_some_and(|line| {
                line.contains("exceeded CUs meter")
                    || line.contains("ComputationalBudgetExceeded")
                    || line.ends_with("failed: Computational budget exceeded")
            })
        })
    })
}

// The pinned program uses this error for `require_gt!(shares, 0)` as well
// as invalid index arithmetic. Recognize only the explicit zero/zero bound;
// all other arithmetic failures remain visible and abort the attempt.
pub(super) fn zero_share_probe(value: &serde_json::Value) -> bool {
    let Some(logs) = value["value"]["logs"].as_array() else {
        return false;
    };
    logs.iter().any(|v| {
        v.as_str()
            .is_some_and(|s| s.contains("Error Code: DebtShareDivisionOverflow."))
    }) && logs.iter().any(|v| v == "Program log: Left: 0")
        && logs.iter().any(|v| v == "Program log: Right: 0")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signing_fingerprint_invalidates_epoch_and_schedule_changes() {
        let mut a = Snapshot {
            slot: 1,
            accounts: Default::default(),
        };
        a.accounts.insert(
            "mint".into(),
            Some(crate::liquidation::plan::Account {
                owner: "token".into(),
                data: vec![1],
                executable: false,
            }),
        );
        let initial = state_hash(&a, 4).unwrap();
        let mut b = a.clone();
        b.slot = 2;
        b.accounts.insert(
            CLOCK.into(),
            Some(crate::liquidation::plan::Account {
                owner: "sysvar".into(),
                data: vec![2],
                executable: false,
            }),
        );
        assert_eq!(initial, state_hash(&b, 4).unwrap());
        assert_ne!(initial, state_hash(&b, 5).unwrap());
        b.accounts.get_mut("mint").unwrap().as_mut().unwrap().data[0] = 2;
        assert_ne!(initial, state_hash(&b, 4).unwrap());
    }
    #[test]
    fn compute_limits_allow_smaller_probes_for_all_rpc_error_forms() {
        for value in [
            serde_json::json!({"value":{"err":{"InstructionError":[4,"ComputationalBudgetExceeded"]},"logs":[]}}),
            serde_json::json!({"value":{"err":"TransactionErrorInstructionError { index: 4, error: ComputationalBudgetExceeded }","logs":[]}}),
            serde_json::json!({"value":{"err":"execution failed","logs":["Program dusk failed: Computational budget exceeded"]}}),
            serde_json::json!({"value":{"err":"execution failed","logs":["exceeded CUs meter at BPF instruction"]}}),
        ] {
            assert!(compute_limit_rejection(&value));
        }
        assert!(!compute_limit_rejection(
            &serde_json::json!({"value":{"err":{"InstructionError":[4,{"Custom":6001}]},"logs":["Error Code: BrokenInvariant."]}})
        ));
    }
    #[test]
    fn zero_share_probe_does_not_hide_other_arithmetic_failures() {
        let mut value = serde_json::json!({"value":{"logs":["Error Code: DebtShareDivisionOverflow.","Program log: Left: 0","Program log: Right: 0"]}});
        assert!(zero_share_probe(&value));
        value["value"]["logs"][2] = serde_json::json!("Program log: Right: 1000000000");
        assert!(!zero_share_probe(&value));
        assert!(!expected_bound_rejection(&value));
    }
}
