//! Token-2022 fee notice monitoring. This never authorizes liquidation.
use serde::Deserialize;

pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const CLOCK: &str = "SysvarC1ock11111111111111111111111111111111";
pub const SYSVAR_OWNER: &str = "Sysvar1111111111111111111111111111111111111";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct TransferFee {
    pub epoch: u64,
    pub bps: u16,
    pub maximum: u64,
}
impl TransferFee {
    pub fn credit(self, gross: u64) -> u64 {
        let fee = (u128::from(gross) * u128::from(self.bps)).div_ceil(10_000);
        gross - fee.min(u128::from(self.maximum)) as u64
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MintFeeSchedule {
    pub older: TransferFee,
    pub newer: TransferFee,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeePhase {
    None,
    Notice,
    FinalNotice,
}
impl MintFeeSchedule {
    pub fn phase(self, epoch: u64) -> FeePhase {
        match self.newer.epoch.checked_sub(epoch) {
            Some(1) => FeePhase::FinalNotice,
            Some(delta) if delta > 1 => FeePhase::Notice,
            _ => FeePhase::None,
        }
    }
    pub fn credit(self, epoch: u64, gross: u64, admission: bool, liquidation: bool) -> u64 {
        let phase = self.phase(epoch);
        let effective = if phase == FeePhase::None {
            self.newer
        } else {
            self.older
        }
        .credit(gross);
        if phase != FeePhase::None && (admission || (liquidation && phase == FeePhase::FinalNotice))
        {
            effective.min(self.newer.credit(gross))
        } else {
            effective
        }
    }
}
fn fee(data: &[u8]) -> Result<TransferFee, String> {
    let fee = TransferFee {
        epoch: u64::from_le_bytes(data[0..8].try_into().unwrap()),
        maximum: u64::from_le_bytes(data[8..16].try_into().unwrap()),
        bps: u16::from_le_bytes(data[16..18].try_into().unwrap()),
    };
    if fee.bps > 10_000 {
        return Err("invalid transfer fee rate".into());
    }
    Ok(fee)
}
/// Decode the published SPL Mint + TLV layout. Validate owner before parsing.
/// Unknown extensions are skipped; malformed or duplicate fee configs fail closed.
pub fn decode_mint_fees(owner: &str, data: &[u8]) -> Result<Option<MintFeeSchedule>, String> {
    if ![TOKEN_PROGRAM, TOKEN_2022_PROGRAM].contains(&owner) || data.len() < 82 || data[45] != 1 {
        return Err("invalid mint identity or initialization".into());
    }
    if data.len() == 82 {
        return Ok(None);
    }
    if owner == TOKEN_PROGRAM || data.len() <= 165 || data.len() == 355 || data[165] != 1 {
        return Err("invalid extended mint".into());
    }
    let mut cursor = 166;
    let mut result = None;
    while cursor < data.len() {
        if data[cursor..].iter().all(|b| *b == 0) {
            break;
        }
        let header = data
            .get(cursor..cursor + 4)
            .ok_or("truncated mint TLV header")?;
        let kind = u16::from_le_bytes(header[..2].try_into().unwrap());
        let len = usize::from(u16::from_le_bytes(header[2..].try_into().unwrap()));
        cursor += 4;
        let value = data
            .get(cursor..cursor + len)
            .ok_or("truncated mint TLV value")?;
        if kind == 1 {
            if result.is_some() || len != 108 {
                return Err("invalid transfer fee config".into());
            }
            result = Some(MintFeeSchedule {
                older: fee(&value[72..90])?,
                newer: fee(&value[90..108])?,
            });
        }
        cursor += len;
    }
    Ok(result)
}
pub fn decode_clock_epoch(owner: &str, data: &[u8], slot: u64) -> Result<u64, String> {
    if owner != SYSVAR_OWNER
        || data.len() != 40
        || u64::from_le_bytes(data[..8].try_into().unwrap()) != slot
    {
        return Err("clock and RPC bank do not match".into());
    }
    Ok(u64::from_le_bytes(data[16..24].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shares_sdk_notice_vectors() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/conformance/v1/transfer-fee-notice.json"
        ))
        .unwrap();
        for v in vectors.as_array().unwrap() {
            let parse = |a: &serde_json::Value| TransferFee {
                epoch: a[0].as_str().unwrap().parse().unwrap(),
                bps: a[1].as_u64().unwrap() as u16,
                maximum: a[2].as_str().unwrap().parse().unwrap(),
            };
            let schedule = MintFeeSchedule {
                older: parse(&v["older"]),
                newer: parse(&v["newer"]),
            };
            let epoch = v["epoch"].as_str().unwrap().parse().unwrap();
            let gross = v["gross"].as_str().unwrap().parse().unwrap();
            let phase = match schedule.phase(epoch) {
                FeePhase::None => "none",
                FeePhase::Notice => "notice",
                FeePhase::FinalNotice => "finalNotice",
            };
            assert_eq!(phase, v["phase"].as_str().unwrap());
            for mode in ["effective", "admission", "liquidation"] {
                assert_eq!(
                    schedule
                        .credit(epoch, gross, mode == "admission", mode == "liquidation")
                        .to_string(),
                    v[mode].as_str().unwrap(),
                    "{}: {mode}",
                    v["name"]
                );
            }
        }
    }
    #[test]
    fn validates_token_owner_tlv_and_clock() {
        let mut data = vec![0; 278];
        data[45] = 1;
        data[165] = 1;
        data[166] = 1;
        data[168] = 108;
        data[260..268].copy_from_slice(&12_u64.to_le_bytes());
        data[276..278].copy_from_slice(&10000_u16.to_le_bytes());
        let fees = decode_mint_fees(TOKEN_2022_PROGRAM, &data)
            .unwrap()
            .unwrap();
        assert_eq!(fees.phase(11), FeePhase::FinalNotice);
        assert!(decode_mint_fees(TOKEN_PROGRAM, &data).is_err());
        assert!(decode_mint_fees("fake", &data).is_err());
        assert!(decode_mint_fees(TOKEN_2022_PROGRAM, &data[..270]).is_err());
        data[45] = 0;
        assert!(decode_mint_fees(TOKEN_2022_PROGRAM, &data).is_err());
        let mut clock = vec![0; 40];
        clock[..8].copy_from_slice(&123_u64.to_le_bytes());
        clock[16..24].copy_from_slice(&11_u64.to_le_bytes());
        assert_eq!(decode_clock_epoch(SYSVAR_OWNER, &clock, 123).unwrap(), 11);
        assert!(decode_clock_epoch(SYSVAR_OWNER, &clock, 124).is_err());
    }
}
