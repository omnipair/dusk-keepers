//! Parameter-proposal lifecycle.
//!
//! Governance moves a proposal through queueing and then execution, each
//! gated on support, timelocks and windows. This keeper does none of that
//! arithmetic. It finds proposals, offers both transitions to the program, and
//! sends whichever the program accepts — every refusal is a proposal that is
//! simply not ready, which on a quiet market is every proposal every pass.
//!
//! That is why the proposal account is barely decoded at all. `ParameterProposal`
//! carries a data-bearing enum partway through, so the fields past it have no
//! fixed offset; the keeper reads only the market ahead of it and lets
//! simulation answer the question the status byte would have answered less
//! reliably.

use std::collections::BTreeMap;

use dusk_adapter::{
    AccountLayoutManifest, DeterministicAccountResolver, InstructionContract,
    KeeperInstructionArguments, PdaSeedValue, encode_keeper_instruction,
};
use keeper_core::{OutcomeStatus, ReasonCode};

use crate::{
    accounts::{AccountAssembler, MarketAccounts, decode_key},
    discovery::{RpcClient, account_discriminator},
    execute::{AttemptReport, ExecutionError, classify_simulation_failure},
    signer::TransactionSigner,
    transaction::{Instruction, base64, compile_message, serialize_transaction},
};

const QUEUE: &str = "dusk:queue_parameter_proposal";
const EXECUTE: &str = "dusk:execute_parameter_proposal";

/// A proposal, as far as the keeper reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProposalRecord {
    pub address: [u8; 32],
    pub market: [u8; 32],
}

impl ProposalRecord {
    pub fn decode(
        layout: &AccountLayoutManifest,
        address: [u8; 32],
        data: &[u8],
    ) -> Option<Self> {
        if data.first_chunk::<8>()? != &account_discriminator("ParameterProposal") {
            return None;
        }
        let reader = layout.reader("ParameterProposal").ok()?;
        Some(Self {
            address,
            market: reader.pubkey("market", data).ok()?,
        })
    }
}

pub struct LifecycleJob<'a> {
    pub client: &'a RpcClient,
    pub contract: &'a InstructionContract,
    pub layout: &'a AccountLayoutManifest,
    pub resolver: &'a DeterministicAccountResolver,
    pub signer: &'a dyn TransactionSigner,
    pub program_id: [u8; 32],
    pub max_sends_per_pass: usize,
    pub dry_run: bool,
}

impl LifecycleJob<'_> {
    pub fn candidates(&self) -> Result<Vec<ProposalRecord>, ExecutionError> {
        let accounts = self.client.program_accounts(
            &bs58::encode(self.program_id).into_string(),
            account_discriminator("ParameterProposal"),
        )?;
        Ok(accounts
            .into_iter()
            .filter_map(|(address, data)| ProposalRecord::decode(self.layout, address, &data))
            .collect())
    }

    fn market_accounts(&self, market: [u8; 32]) -> Result<MarketAccounts, ExecutionError> {
        let data = self
            .client
            .account_data(&bs58::encode(market).into_string())?;
        MarketAccounts::decode(self.layout, market, &data).ok_or_else(|| {
            ExecutionError::Assembly(format!(
                "{} is not a market account",
                bs58::encode(market).into_string()
            ))
        })
    }

    fn instruction(
        &self,
        key: &str,
        proposal: &ProposalRecord,
        market: &MarketAccounts,
    ) -> Result<Instruction, ExecutionError> {
        let mut supplied: BTreeMap<&'static str, [u8; 32]> = BTreeMap::new();
        supplied.insert("market", market.address);
        supplied.insert("proposal", proposal.address);
        // Queueing weighs support held as hLP, so it names the yLP mint and
        // both hLP yLP vaults; execution does not, and the assembler ignores
        // what an instruction does not declare.
        supplied.insert("ylp_mint", market.ylp_mint);
        supplied.insert("base_hlp_ylp_vault", market.base_ylp_vault);
        supplied.insert("quote_hlp_ylp_vault", market.quote_ylp_vault);

        let mut pda_inputs = BTreeMap::new();
        pda_inputs.insert(
            "market".to_owned(),
            PdaSeedValue::Pubkey(bs58::encode(market.address).into_string()),
        );

        let accounts = AccountAssembler {
            contract: self.contract,
            omitted: &[],
            pda_inputs,
            resolver: self.resolver,
            supplied,
        }
        .assemble(key)
        .map_err(|error| ExecutionError::Assembly(error.to_string()))?;

        let arguments = if key == QUEUE {
            KeeperInstructionArguments::QueueParameterProposal
        } else {
            KeeperInstructionArguments::ExecuteParameterProposal
        };
        let data = encode_keeper_instruction(self.contract, &arguments)
            .map_err(|error| ExecutionError::Encoding(error.to_string()))?;

        let program_id = decode_key(
            &self
                .contract
                .instructions
                .iter()
                .find(|entry| entry.key == key)
                .ok_or_else(|| ExecutionError::Encoding(format!("{key} is not in the contract")))?
                .program_id,
        )
        .ok_or_else(|| ExecutionError::Assembly("program id does not decode".into()))?;

        Ok(Instruction {
            accounts,
            data,
            program_id,
        })
    }

    fn encode(&self, instruction: Instruction) -> Result<String, ExecutionError> {
        let blockhash = self.client.latest_blockhash()?;
        let message = compile_message(self.signer.public_key(), &[instruction], blockhash)
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let signature = self.signer.sign(&message);
        Ok(base64(&serialize_transaction(&[signature], &message)))
    }

    fn attempt(
        &self,
        key: &str,
        proposal: &ProposalRecord,
        market: &MarketAccounts,
    ) -> Result<AttemptReport, ExecutionError> {
        let proposal_key = bs58::encode(proposal.address).into_string();
        let encoded = self.encode(self.instruction(key, proposal, market)?)?;

        if let Some(detail) = self.client.simulate(&encoded)? {
            let (reason, race) = classify_simulation_failure(&detail);
            return Ok(AttemptReport {
                debt_mint: key.to_owned(),
                detail: Some(detail),
                position: proposal_key,
                race,
                // A proposal that is not yet due is the ordinary case, not a
                // fault, so an unrecognized governance refusal is reported as
                // bounds rather than raising an alert every pass.
                reason: if reason == ReasonCode::SimulationRejected {
                    ReasonCode::BoundsNotMet
                } else {
                    reason
                },
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        if self.dry_run {
            return Ok(AttemptReport {
                debt_mint: key.to_owned(),
                detail: Some("simulation passed; shadow mode does not send".into()),
                position: proposal_key,
                race: None,
                reason: ReasonCode::ShadowMode,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let encoded = self.encode(self.instruction(key, proposal, market)?)?;
        match self.client.send_transaction(&encoded) {
            Ok(signature) => Ok(AttemptReport {
                debt_mint: key.to_owned(),
                detail: None,
                position: proposal_key,
                race: None,
                reason: ReasonCode::Confirmed,
                signature: Some(signature),
                status: OutcomeStatus::Executed,
            }),
            Err(error) => {
                let detail = error.to_string();
                let (reason, race) = classify_simulation_failure(&detail);
                Ok(AttemptReport {
                    debt_mint: key.to_owned(),
                    detail: Some(detail),
                    position: proposal_key,
                    race,
                    reason,
                    signature: None,
                    status: OutcomeStatus::RetryableFailure,
                })
            }
        }
    }

    pub fn run_pass(&self) -> Result<Vec<AttemptReport>, ExecutionError> {
        let mut reports = Vec::new();
        let mut sent = 0_usize;
        let mut markets: BTreeMap<[u8; 32], MarketAccounts> = BTreeMap::new();

        for proposal in self.candidates()? {
            let market = match markets.entry(proposal.market) {
                std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    *entry.insert(self.market_accounts(proposal.market)?)
                }
            };
            // Execution is offered first: a proposal ready to execute has
            // already been queued, and offering the queue transition to it
            // would only produce a refusal.
            for key in [EXECUTE, QUEUE] {
                if sent >= self.max_sends_per_pass {
                    break;
                }
                let report = self.attempt(key, &proposal, &market)?;
                let executed = report.status == OutcomeStatus::Executed;
                reports.push(report);
                if executed {
                    sent += 1;
                    break;
                }
            }
        }
        Ok(reports)
    }
}
