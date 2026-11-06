//! Permissionless execution of the market's single announced operator bundle.
//! Ownership successors sign acceptance themselves. Revenue rotation archives
//! collected fees atomically before execution; all eligibility remains on-chain.
use crate::{
    liquidation::{
        plan::{Snapshot, address, key},
        simulation, state_hash,
    },
    runtime::{
        accounts::{AccountAssembler, MarketAccounts},
        execution::{AttemptReport, ExecutionError},
        flash_journal::{Journal, recover_pending},
        rpc::account_discriminator,
        transaction::Instruction,
    },
};
use dusk_adapter::{
    AccountLayoutManifest, KeeperInstructionArguments as Args, derive_program_address,
    encode_keeper_instruction,
};
use keeper_core::{OutcomeStatus, ReasonCode};
use std::collections::BTreeMap;

const EXECUTE: &str = "dusk:execute_operator_change";
const ARCHIVE: &str = "dusk:initialize_operator_revenue";

#[derive(Debug, PartialEq)]
struct Notice {
    market: [u8; 32],
    nonce: u64,
    epoch: u64,
    next_operator: Option<[u8; 32]>,
    recipient: Option<[u8; 32]>,
}
impl Notice {
    fn decode(layout: &AccountLayoutManifest, data: &[u8]) -> Option<Self> {
        if data.get(..8)? != account_discriminator("OperatorProposal") {
            return None;
        }
        let reader = layout.reader("OperatorProposal").ok()?;
        let dynamic = layout
            .accounts
            .iter()
            .find(|row| row.name == "OperatorProposal")?
            .dynamic
            .as_ref()?;
        let mut at = dynamic.updates_offset;
        let count = u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
        at += 4;
        if count > dynamic.variant_payload_sizes.len() {
            return None;
        }
        for _ in 0..count {
            let variant = *data.get(at)? as usize;
            at = at.checked_add(1 + *dynamic.variant_payload_sizes.get(variant)?)?;
            data.get(..at)?;
        }
        let mut optional_key = || -> Option<Option<[u8; 32]>> {
            let tag = *data.get(at)?;
            at += 1;
            match tag {
                0 => Some(None),
                1 => {
                    let value = data.get(at..at + 32)?.try_into().ok()?;
                    at += 32;
                    Some(Some(value))
                }
                _ => None,
            }
        };
        let next_operator = optional_key()?;
        let _next_manager = optional_key()?;
        let recipient = optional_key()?;
        Some(Self {
            market: reader.pubkey("market", data).ok()?,
            nonce: reader.u64("nonce", data).ok()?,
            epoch: reader.u64("epoch", data).ok()?,
            next_operator,
            recipient,
        })
    }
}

fn instructions(
    context: &super::WorkerContext<'_>,
    snapshot: &Snapshot,
    market_key: [u8; 32],
    proposal: [u8; 32],
) -> Result<Vec<Instruction>, String> {
    let data = snapshot.owned(market_key, context.program_id)?;
    if data.get(..8) != Some(account_discriminator("Market").as_slice()) {
        return Err("not a market".into());
    }
    let reader = context.layout.reader("Market").map_err(|e| e.to_string())?;
    if reader
        .pubkey("operator.pending", data)
        .map_err(|e| e.to_string())?
        != proposal
    {
        return Err("operator bundle replaced".into());
    }
    let notice = Notice::decode(
        context.layout,
        snapshot.owned(proposal, context.program_id)?,
    )
    .ok_or("invalid operator notice")?;
    if notice.market != market_key
        || notice.epoch
            != reader
                .u64("operator.epoch", data)
                .map_err(|e| e.to_string())?
    {
        return Err("operator epoch changed".into());
    }
    let expected = derive_program_address(
        &[
            b"operator_change".to_vec(),
            market_key.to_vec(),
            notice.nonce.to_le_bytes().to_vec(),
        ],
        &address(context.program_id),
    )
    .map_err(|e| e.to_string())?;
    if key(&expected.address)? != proposal {
        return Err("invalid operator proposal PDA".into());
    }
    let accepting = notice.next_operator.filter(|key| *key != [0; 32]);
    if accepting.is_some_and(|key| key != context.signer.public_key()) {
        return Err("successor acceptance requires its own signer".into());
    }
    let current_recipient = reader
        .pubkey("operator.revenue_recipient", data)
        .map_err(|e| e.to_string())?;
    let next_recipient = notice.recipient.or(accepting).unwrap_or(current_recipient);
    let rotation = current_recipient != next_recipient;
    let epoch = reader
        .u64("operator.revenue_epoch", data)
        .map_err(|e| e.to_string())?;
    let revenue = key(&derive_program_address(
        &[
            b"operator_revenue".to_vec(),
            market_key.to_vec(),
            epoch.to_le_bytes().to_vec(),
        ],
        &address(context.program_id),
    )
    .map_err(|e| e.to_string())?
    .address)?;
    let market =
        MarketAccounts::decode(context.layout, market_key, data).ok_or("invalid market layout")?;
    let build = |name: &str, args: Args, omit: &[&str]| -> Result<Instruction, String> {
        let mut supplied = BTreeMap::from([
            ("market", market_key),
            ("proposal", proposal),
            ("payer", context.signer.public_key()),
            ("revenue", revenue),
            ("previous_revenue", revenue),
        ]);
        if let Some(signer) = accepting {
            supplied.insert("accepting_operator", signer);
        }
        let mut accounts = AccountAssembler {
            contract: context.contract,
            resolver: context.resolver,
            supplied,
            omitted: omit,
            pda_inputs: BTreeMap::new(),
        }
        .assemble(name)
        .map_err(|e| e.to_string())?;
        if name == EXECUTE {
            accounts.extend(market.oracle_remaining_accounts());
        }
        Ok(Instruction {
            program_id: context.program_id,
            accounts,
            data: encode_keeper_instruction(context.contract, &args).map_err(|e| e.to_string())?,
        })
    };
    let mut result = vec![Instruction {
        program_id: key("ComputeBudget111111111111111111111111111111")?,
        accounts: vec![],
        data: [vec![2], 1_400_000_u32.to_le_bytes().to_vec()].concat(),
    }];
    if rotation
        && !snapshot
            .accounts
            .get(&address(revenue))
            .is_some_and(Option::is_some)
    {
        result.push(build(ARCHIVE, Args::InitializeOperatorRevenue, &[])?);
    }
    let mut omit = vec![];
    if !rotation {
        omit.push("previous_revenue");
    }
    if accepting.is_none() {
        omit.push("accepting_operator");
    }
    result.push(build(EXECUTE, Args::ExecuteOperatorChange, &omit)?);
    Ok(result)
}

pub fn run(context: &super::WorkerContext<'_>) -> Result<Vec<AttemptReport>, ExecutionError> {
    let mut reports = recover_pending(
        context.client,
        &context.contract.protocol_revision,
        context.dry_run,
    )?;
    let markets = context.client.program_accounts(
        &address(context.program_id),
        account_discriminator("Market"),
    )?;
    let mut attempts = 0;
    for (market, data) in markets {
        let reader = context
            .layout
            .reader("Market")
            .map_err(|e| ExecutionError::Assembly(e.to_string()))?;
        let Ok(proposal) = reader.pubkey("operator.pending", &data) else {
            continue;
        };
        if proposal == [0; 32] {
            continue;
        }
        if attempts >= context.max_sends {
            break;
        }
        attempts += 1;
        let result = (|| -> Result<Option<String>, String> {
            let first = Snapshot::fetch(context.client, &[market, proposal], 0)
                .map_err(|e| e.to_string())?;
            let initial = instructions(context, &first, market, proposal)?;
            let mut keys = initial
                .iter()
                .flat_map(|ix| ix.accounts.iter().map(|meta| meta.pubkey))
                .collect::<Vec<_>>();
            keys.extend([market, proposal]);
            keys.sort();
            keys.dedup();
            let snapshot =
                Snapshot::fetch(context.client, &keys, first.slot).map_err(|e| e.to_string())?;
            let plan = instructions(context, &snapshot, market, proposal)?;
            let simulated = simulation::simulate_for(
                context.client,
                context.signer.public_key(),
                &snapshot,
                &plan,
                &[market],
            )?;
            if !simulated["value"]["err"].is_null() {
                return Err(format!(
                    "operator bundle not executable: {}",
                    simulated["value"]
                ));
            }
            if context.dry_run {
                return Ok(None);
            }
            let fresh =
                Snapshot::fetch(context.client, &keys, snapshot.slot).map_err(|e| e.to_string())?;
            if state_hash(&snapshot, 0)? != state_hash(&fresh, 0)? {
                return Err("operator execution state changed; re-evaluate".into());
            }
            let plan = instructions(context, &fresh, market, proposal)?;
            if !simulation::simulate_for(
                context.client,
                context.signer.public_key(),
                &fresh,
                &plan,
                &[market],
            )?["value"]["err"]
                .is_null()
            {
                return Err("operator pre-submit simulation rejected".into());
            }
            let block: serde_json::Value = context
                .client
                .call(
                    "getLatestBlockhash",
                    serde_json::json!([{"commitment":"confirmed","minContextSlot":fresh.slot}]),
                )
                .map_err(|e| e.to_string())?;
            let hash = block["value"]["blockhash"]
                .as_str()
                .ok_or("blockhash missing")?;
            let height = block["value"]["lastValidBlockHeight"]
                .as_u64()
                .ok_or("block height missing")?;
            let message = simulation::compile_message_for(
                context.signer.public_key(),
                &fresh,
                &plan,
                key(hash)?,
            )?;
            let mut journal = Journal::connect(
                &std::env::var("DATABASE_URL")
                    .map_err(|_| "DATABASE_URL is required before signing")?,
            )?;
            journal.submit(
                context.client,
                context.signer,
                &context.contract.protocol_revision,
                &address(proposal),
                0,
                &address(market),
                "operator_bundle_execute",
                fresh.slot,
                &state_hash(&fresh, 0)?,
                &message,
                hash,
                height,
            )
        })();
        let (signature, detail, status, reason) = match result {
            Ok(Some(sig)) => (
                Some(sig),
                None,
                OutcomeStatus::Executed,
                ReasonCode::Confirmed,
            ),
            Ok(None) => (
                None,
                Some("simulation passed; no new signature produced".into()),
                OutcomeStatus::Skipped,
                if context.dry_run {
                    ReasonCode::ShadowMode
                } else {
                    ReasonCode::AlreadyResolved
                },
            ),
            Err(error) => {
                let expected = [
                    "bundle replaced",
                    "epoch changed",
                    "successor acceptance",
                    "OperatorNoticeNotReady",
                    "OperatorProposalStale",
                    "OperatorProposalExpired",
                    "UtilizationGuardExceeded",
                    "state changed; re-evaluate",
                ]
                .iter()
                .any(|name| error.contains(name));
                (
                    None,
                    Some(error),
                    if expected {
                        OutcomeStatus::Skipped
                    } else {
                        OutcomeStatus::RetryableFailure
                    },
                    if expected {
                        ReasonCode::BoundsNotMet
                    } else {
                        ReasonCode::SimulationRejected
                    },
                )
            }
        };
        reports.push(AttemptReport {
            position: address(proposal),
            debt_mint: EXECUTE.into(),
            signature,
            detail,
            status,
            reason,
            race: None,
        });
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MANIFEST: &str = include_str!("../../../../../../protocol/keeper-account-layout.v1.json");
    #[test]
    fn proposal_suffix_uses_generated_variant_sizes_and_strict_option_tags() {
        let layout = AccountLayoutManifest::from_json(MANIFEST).unwrap();
        let dynamic = layout
            .accounts
            .iter()
            .find(|a| a.name == "OperatorProposal")
            .unwrap()
            .dynamic
            .as_ref()
            .unwrap();
        let mut bytes = vec![0; dynamic.updates_offset];
        bytes[..8].copy_from_slice(&account_discriminator("OperatorProposal"));
        bytes[8..40].fill(7);
        bytes[72..80].copy_from_slice(&19u64.to_le_bytes());
        bytes[80..88].copy_from_slice(&23u64.to_le_bytes());
        bytes.extend_from_slice(&7u32.to_le_bytes());
        for (variant, size) in dynamic.variant_payload_sizes.iter().enumerate() {
            bytes.push(variant as u8);
            bytes.resize(bytes.len() + size, 0);
        }
        let suffix = bytes.len();
        bytes.push(1);
        bytes.extend_from_slice(&[11; 32]);
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&[13; 32]);
        assert_eq!(
            Notice::decode(&layout, &bytes),
            Some(Notice {
                market: [7; 32],
                epoch: 19,
                nonce: 23,
                next_operator: Some([11; 32]),
                recipient: Some([13; 32])
            })
        );
        for end in 0..bytes.len() {
            assert_eq!(Notice::decode(&layout, &bytes[..end]), None);
        }
        bytes[suffix] = 2;
        assert_eq!(Notice::decode(&layout, &bytes), None);
        bytes[suffix] = 1;
        bytes[dynamic.updates_offset + 4] = 255;
        assert_eq!(Notice::decode(&layout, &bytes), None);
    }
}
