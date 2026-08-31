//! Liquidating a leverage position.
//!
//! Structurally the trigger's sibling rather than the bidder's: there is no
//! auction to price, so nothing has to be measured. The position is either
//! liquidatable or it is not, and the program decides which.
//!
//! Unlike a borrow position, a leverage position records which side carries
//! its debt, so the keeper reads it rather than offering both and paying a
//! simulation to be told. That also keeps the log honest: probing the wrong
//! side produced a refusal that classified as a state change, which reads as
//! a race that never happened.
//!
//! Three of this instruction's accounts have PDA recipes that the resolution
//! manifest registers only against the delegated-close path, because the IDL
//! does not declare seeds for them here. They are derived from the same
//! recipes explicitly rather than left to the assembler, so the derivation
//! stays the manifest's rather than becoming this file's.

use std::collections::BTreeMap;

use dusk_adapter::{
    AccountLayoutManifest, DeterministicAccountResolver, InstructionContract,
    KeeperInstructionArguments, LiquidateLeveragePositionArgs, PdaSeedValue,
    encode_keeper_instruction,
};
use keeper_core::{OutcomeStatus, ReasonCode};

use crate::{
    accounts::{AccountAssembler, MarketAccounts, associated_token_account, decode_key},
    discovery::{RpcClient, account_discriminator},
    execute::{AttemptReport, ExecutionError, classify_simulation_failure},
    signer::TransactionSigner,
    transaction::{Instruction, base64, compile_message, serialize_transaction},
};

const INSTRUCTION_KEY: &str = "dusk:liquidate_leverage_position";

/// A leverage position, as far as the keeper reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeveragePositionRecord {
    pub address: [u8; 32],
    pub owner: [u8; 32],
    pub market: [u8; 32],
    pub position_id: [u8; 32],
    /// Which market side the position owes. Zero is base, matching the
    /// program's enum ordering.
    pub debt_asset: u8,
}

impl LeveragePositionRecord {
    pub fn debt_is_base(&self) -> bool {
        self.debt_asset == 0
    }

    pub fn decode(
        layout: &AccountLayoutManifest,
        address: [u8; 32],
        data: &[u8],
    ) -> Option<Self> {
        if data.first_chunk::<8>()? != &account_discriminator("LeveragePosition") {
            return None;
        }
        let reader = layout.reader("LeveragePosition").ok()?;
        Some(Self {
            address,
            market: reader.pubkey("market", data).ok()?,
            owner: reader.pubkey("owner", data).ok()?,
            debt_asset: reader.u8("debt_asset", data).ok()?,
            position_id: reader.pubkey("position_id", data).ok()?,
        })
    }
}

pub struct LeverageJob<'a> {
    pub client: &'a RpcClient,
    pub contract: &'a InstructionContract,
    pub layout: &'a AccountLayoutManifest,
    pub resolver: &'a DeterministicAccountResolver,
    pub signer: &'a dyn TransactionSigner,
    pub program_id: [u8; 32],
    pub max_sends_per_pass: usize,
    pub dry_run: bool,
}

impl LeverageJob<'_> {
    pub fn candidates(&self) -> Result<Vec<LeveragePositionRecord>, ExecutionError> {
        let accounts = self.client.program_accounts(
            &bs58::encode(self.program_id).into_string(),
            account_discriminator("LeveragePosition"),
        )?;
        Ok(accounts
            .into_iter()
            .filter_map(|(address, data)| {
                LeveragePositionRecord::decode(self.layout, address, &data)
            })
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

    fn derive(
        &self,
        recipe: &str,
        inputs: BTreeMap<String, PdaSeedValue>,
    ) -> Result<[u8; 32], ExecutionError> {
        let resolved = self
            .resolver
            .derive_pda(recipe, &inputs)
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        decode_key(&resolved.address)
            .ok_or_else(|| ExecutionError::Assembly(format!("{recipe} did not decode")))
    }

    fn liquidate_instruction(
        &self,
        position: &LeveragePositionRecord,
        market: &MarketAccounts,
        debt_is_base: bool,
    ) -> Result<Instruction, ExecutionError> {
        let sides = market.oriented(debt_is_base);
        let signer = self.signer.public_key();

        let token_program = decode_key(
            self.resolver
                .resolve_static(INSTRUCTION_KEY, "token_program")
                .ok_or_else(|| {
                    ExecutionError::Assembly("token program is not in the manifest".into())
                })?,
        )
        .ok_or_else(|| ExecutionError::Assembly("token program does not decode".into()))?;

        let futarchy_authority = self.derive("dusk:futarchy_authority", BTreeMap::new())?;
        let leverage_collateral_vault = self.derive(
            "dusk:leverage_collateral_vault",
            BTreeMap::from([
                (
                    "market".to_owned(),
                    PdaSeedValue::Pubkey(bs58::encode(market.address).into_string()),
                ),
                (
                    "collateralMint".to_owned(),
                    PdaSeedValue::Pubkey(bs58::encode(sides.collateral_mint).into_string()),
                ),
            ]),
        )?;

        let liquidator_debt_account =
            associated_token_account(signer, token_program, sides.debt_mint).ok_or_else(|| {
                ExecutionError::Assembly("liquidator debt account does not derive".into())
            })?;
        let owner_debt_account =
            associated_token_account(position.owner, token_program, sides.debt_mint).ok_or_else(
                || ExecutionError::Assembly("owner debt account does not derive".into()),
            )?;

        let mut supplied: BTreeMap<&'static str, [u8; 32]> = BTreeMap::new();
        supplied.insert("market", market.address);
        supplied.insert("futarchy_authority", futarchy_authority);
        supplied.insert("position_owner", position.owner);
        supplied.insert("leverage_position", position.address);
        supplied.insert("debt_mint", sides.debt_mint);
        supplied.insert("collateral_mint", sides.collateral_mint);
        supplied.insert("debt_reserve_vault", sides.debt_reserve_vault);
        supplied.insert("collateral_reserve_vault", sides.collateral_reserve_vault);
        supplied.insert("debt_interest_vault", sides.debt_interest_vault);
        supplied.insert("leverage_collateral_vault", leverage_collateral_vault);
        supplied.insert("liquidator_debt_account", liquidator_debt_account);
        supplied.insert("owner_debt_account", owner_debt_account);
        supplied.insert("liquidator", signer);

        let accounts = AccountAssembler {
            contract: self.contract,
            omitted: &["referral_partner", "referral_accrual"],
            pda_inputs: BTreeMap::new(),
            resolver: self.resolver,
            supplied,
        }
        .assemble(INSTRUCTION_KEY)
        .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let accounts = [accounts, market.hlp_remaining_accounts()].concat();

        let data = encode_keeper_instruction(
            self.contract,
            &KeeperInstructionArguments::LiquidateLeveragePosition(
                LiquidateLeveragePositionArgs {
                    debt_asset: u8::from(!debt_is_base),
                },
            ),
        )
        .map_err(|error| ExecutionError::Encoding(error.to_string()))?;

        let program_id = decode_key(
            &self
                .contract
                .instructions
                .iter()
                .find(|entry| entry.key == INSTRUCTION_KEY)
                .ok_or_else(|| {
                    ExecutionError::Encoding("leverage liquidation is not in the contract".into())
                })?
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
        position: &LeveragePositionRecord,
        market: &MarketAccounts,
        debt_is_base: bool,
    ) -> Result<AttemptReport, ExecutionError> {
        let position_key = bs58::encode(position.address).into_string();
        let side = if debt_is_base { "base" } else { "quote" };
        let encoded = self.encode(self.liquidate_instruction(position, market, debt_is_base)?)?;

        if let Some(detail) = self.client.simulate(&encoded)? {
            let (reason, race) = classify_simulation_failure(&detail);
            return Ok(AttemptReport {
                debt_mint: side.to_owned(),
                detail: Some(detail),
                position: position_key,
                race,
                reason,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        if self.dry_run {
            return Ok(AttemptReport {
                debt_mint: side.to_owned(),
                detail: Some("simulation passed; shadow mode does not send".into()),
                position: position_key,
                race: None,
                reason: ReasonCode::ShadowMode,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let encoded = self.encode(self.liquidate_instruction(position, market, debt_is_base)?)?;
        match self.client.send_transaction(&encoded) {
            Ok(signature) => Ok(AttemptReport {
                debt_mint: side.to_owned(),
                detail: None,
                position: position_key,
                race: None,
                reason: ReasonCode::Confirmed,
                signature: Some(signature),
                status: OutcomeStatus::Executed,
            }),
            Err(error) => {
                let detail = error.to_string();
                let (reason, race) = classify_simulation_failure(&detail);
                Ok(AttemptReport {
                    debt_mint: side.to_owned(),
                    detail: Some(detail),
                    position: position_key,
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

        for position in self.candidates()? {
            let market = match markets.entry(position.market) {
                std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    *entry.insert(self.market_accounts(position.market)?)
                }
            };
            if sent >= self.max_sends_per_pass {
                break;
            }
            let report = self.attempt(&position, &market, position.debt_is_base())?;
            if report.status == OutcomeStatus::Executed {
                sent += 1;
            }
            reports.push(report);
        }
        Ok(reports)
    }
}
