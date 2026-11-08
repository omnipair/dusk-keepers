//! Settling protocol revenue auctions.
//!
//! The protocol accrues revenue in whichever asset produced it and sells that
//! for the lane's accepted asset, paying the proceeds to the treasury and the
//! staking vault. This keeper is the buyer of last resort for those sales.
//!
//! It cannot know in advance which lane, revenue source or market side has
//! anything to sell — that depends on accrued fee liabilities the keeper does
//! not model — so it offers every combination and lets the program refuse the
//! empty ones. On a market that has not accrued revenue, which is every market
//! until trading starts, all eight refuse, and that is the system working.
//!
//! Payment is measured rather than predicted, as in the bidder: simulate with
//! a ceiling high enough not to bind, read what the sale would actually cost,
//! then bind the sent transaction to that.

use std::{collections::BTreeMap, env};

use dusk_adapter::{
    AccountLayoutManifest, DeterministicAccountResolver, InstructionContract,
    KeeperInstructionArguments, PdaSeedValue, ProtocolAuctionLane, ProtocolRevenueSource,
    SettleProtocolAuctionArgs, encode_keeper_instruction,
};
use keeper_core::{OutcomeStatus, ReasonCode};

use crate::{
    liquidation::{
        plan::{Snapshot, address, key, token_amount},
        simulation, state_hash,
    },
    runtime::accounts::{AccountAssembler, MarketAccounts, associated_token_account, decode_key},
    runtime::execution::{AttemptReport, ExecutionError, classify_simulation_failure},
    runtime::flash_journal::{Journal, recover_pending},
    runtime::rpc::{RpcClient, account_discriminator},
    runtime::signer::TransactionSigner,
    runtime::transaction::{Instruction, base64, compile_message, serialize_transaction},
};

const INSTRUCTION_KEY: &str = "dusk:settle_protocol_auction";

/// A lane's accepted asset and where its proceeds go.
#[derive(Clone, Copy, Debug)]
pub struct LaneConfig {
    pub accepted_mint: [u8; 32],
    pub treasury: [u8; 32],
    pub staking_vault: [u8; 32],
}

#[derive(Clone, Copy, Debug)]
pub struct FutarchyConfig {
    pub address: [u8; 32],
    pub fee: LaneConfig,
    pub buyback: LaneConfig,
}

impl FutarchyConfig {
    pub fn decode(layout: &AccountLayoutManifest, address: [u8; 32], data: &[u8]) -> Option<Self> {
        if data.first_chunk::<8>()? != &account_discriminator("FutarchyAuthority") {
            return None;
        }
        let reader = layout.reader("FutarchyAuthority").ok()?;
        Some(Self {
            address,
            buyback: LaneConfig {
                accepted_mint: reader.pubkey("buyback_auction.accepted_mint", data).ok()?,
                staking_vault: reader
                    .pubkey("buyback_auction.recipients.staking_vault", data)
                    .ok()?,
                treasury: reader
                    .pubkey("buyback_auction.recipients.treasury", data)
                    .ok()?,
            },
            fee: LaneConfig {
                accepted_mint: reader.pubkey("fee_auction.accepted_mint", data).ok()?,
                staking_vault: reader
                    .pubkey("fee_auction.recipients.staking_vault", data)
                    .ok()?,
                treasury: reader
                    .pubkey("fee_auction.recipients.treasury", data)
                    .ok()?,
            },
        })
    }

    fn lane(&self, lane: ProtocolAuctionLane) -> LaneConfig {
        match lane {
            ProtocolAuctionLane::Buyback => self.buyback,
            ProtocolAuctionLane::Fee => self.fee,
        }
    }
}

pub struct ArbitragePolicy {
    /// Most revenue to buy in one settlement, in raw atoms of the sold asset.
    pub max_sold: u64,
    /// How far above the measured payment the sent ceiling sits, so a price
    /// that moves a step between simulation and inclusion does not reject it.
    pub slippage_bps: u32,
}

pub struct ArbitrageurJob<'a> {
    pub client: &'a RpcClient,
    pub contract: &'a InstructionContract,
    pub layout: &'a AccountLayoutManifest,
    pub resolver: &'a DeterministicAccountResolver,
    pub signer: &'a dyn TransactionSigner,
    pub program_id: [u8; 32],
    pub policy: ArbitragePolicy,
    pub max_sends_per_pass: usize,
    pub dry_run: bool,
}

impl ArbitrageurJob<'_> {
    fn futarchy(&self) -> Result<FutarchyConfig, ExecutionError> {
        let resolved = self
            .resolver
            .derive_pda("dusk:futarchy_authority", &BTreeMap::new())
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let address = decode_key(&resolved.address)
            .ok_or_else(|| ExecutionError::Assembly("futarchy authority did not decode".into()))?;
        let data = self.client.account_data(&resolved.address)?;
        FutarchyConfig::decode(self.layout, address, &data)
            .ok_or_else(|| ExecutionError::Assembly("futarchy authority did not decode".into()))
    }

    pub fn markets(&self) -> Result<Vec<MarketAccounts>, ExecutionError> {
        let accounts = self.client.program_accounts(
            &bs58::encode(self.program_id).into_string(),
            account_discriminator("Market"),
        )?;
        Ok(accounts
            .into_iter()
            .filter(|(address, _)| crate::runtime::devnet::market_allowed(address))
            .filter_map(|(address, data)| MarketAccounts::decode(self.layout, address, &data))
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn settle_instruction(
        &self,
        market: &MarketAccounts,
        futarchy: &FutarchyConfig,
        lane: ProtocolAuctionLane,
        source: ProtocolRevenueSource,
        sold_is_base: bool,
        max_payment_amount: u64,
    ) -> Result<(Instruction, [u8; 32]), ExecutionError> {
        let config = futarchy.lane(lane);
        let signer = self.signer.public_key();
        let sold_mint = if sold_is_base {
            market.base_mint
        } else {
            market.quote_mint
        };
        // Swap revenue sits in the side's reserve vault; interest revenue in
        // its interest vault. The program checks this, so getting it wrong
        // costs a simulation rather than a mistake.
        let sold_vault = match (source, sold_is_base) {
            (ProtocolRevenueSource::Swap, true) => market.base_reserve_vault,
            (ProtocolRevenueSource::Swap, false) => market.quote_reserve_vault,
            (ProtocolRevenueSource::Interest, true) => market.base_interest_vault,
            (ProtocolRevenueSource::Interest, false) => market.quote_interest_vault,
        };

        let token_program = decode_key(
            self.resolver
                .resolve_static(INSTRUCTION_KEY, "token_program")
                .ok_or_else(|| {
                    ExecutionError::Assembly("token program is not in the manifest".into())
                })?,
        )
        .ok_or_else(|| ExecutionError::Assembly("token program does not decode".into()))?;

        let account_for = |owner: [u8; 32], mint: [u8; 32]| {
            associated_token_account(owner, token_program, mint)
                .ok_or_else(|| ExecutionError::Assembly("token account does not derive".into()))
        };
        let bidder_payment_account = account_for(signer, config.accepted_mint)?;

        let mut supplied: BTreeMap<&'static str, [u8; 32]> = BTreeMap::new();
        supplied.insert("bidder", signer);
        supplied.insert("market", market.address);
        supplied.insert("futarchy_authority", futarchy.address);
        supplied.insert("sold_mint", sold_mint);
        supplied.insert("accepted_mint", config.accepted_mint);
        supplied.insert("sold_vault", sold_vault);
        supplied.insert("bidder_payment_account", bidder_payment_account);
        supplied.insert("bidder_receive_account", account_for(signer, sold_mint)?);
        supplied.insert(
            "treasury_payment_account",
            account_for(config.treasury, config.accepted_mint)?,
        );
        supplied.insert(
            "staking_vault_payment_account",
            account_for(config.staking_vault, config.accepted_mint)?,
        );
        // Only one reference market is permitted per lane, and for a sale
        // between this market's own pair it is this market.
        supplied.insert("reference_market", market.address);

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
        .assemble(INSTRUCTION_KEY)
        .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        let accounts = [accounts, market.hlp_remaining_accounts()].concat();

        let data = encode_keeper_instruction(
            self.contract,
            &KeeperInstructionArguments::SettleProtocolAuction(SettleProtocolAuctionArgs {
                lane,
                max_payment_amount,
                sold_amount: self.policy.max_sold,
                source,
            }),
        )
        .map_err(|error| ExecutionError::Encoding(error.to_string()))?;

        let program_id = decode_key(
            &self
                .contract
                .instructions
                .iter()
                .find(|entry| entry.key == INSTRUCTION_KEY)
                .ok_or_else(|| {
                    ExecutionError::Encoding("protocol auction is not in the contract".into())
                })?
                .program_id,
        )
        .ok_or_else(|| ExecutionError::Assembly("program id does not decode".into()))?;

        Ok((
            Instruction {
                accounts,
                data,
                program_id,
            },
            bidder_payment_account,
        ))
    }

    fn encode(&self, instruction: Instruction) -> Result<String, ExecutionError> {
        let blockhash = self.client.latest_blockhash()?;
        let message = compile_message(self.signer.public_key(), &[instruction], blockhash)
            .map_err(|error| ExecutionError::Assembly(error.to_string()))?;
        // Price measurement never signs. The durable journal owns signing.
        Ok(base64(&serialize_transaction(&[[0; 64]], &message)))
    }

    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &self,
        market: &MarketAccounts,
        futarchy: &FutarchyConfig,
        lane: ProtocolAuctionLane,
        source: ProtocolRevenueSource,
        sold_is_base: bool,
    ) -> Result<AttemptReport, ExecutionError> {
        let label = format!(
            "{}/{}/{}",
            if matches!(lane, ProtocolAuctionLane::Fee) {
                "fee"
            } else {
                "buyback"
            },
            if matches!(source, ProtocolRevenueSource::Swap) {
                "swap"
            } else {
                "interest"
            },
            if sold_is_base { "base" } else { "quote" },
        );
        let market_key = bs58::encode(market.address).into_string();

        // A ceiling high enough not to bind, so the measurement is of the
        // program's price rather than of this keeper's guess at it.
        let (measuring, payment_account) =
            self.settle_instruction(market, futarchy, lane, source, sold_is_base, u64::MAX)?;
        let payment_key = bs58::encode(payment_account).into_string();
        let before = self
            .client
            .account_data(&payment_key)
            .ok()
            .as_deref()
            .and_then(token_amount)
            .unwrap_or(0);

        let encoded = self.encode(measuring)?;
        let (failure, accounts) = self
            .client
            .simulate_with_accounts(&encoded, std::slice::from_ref(&payment_key))?;

        if let Some(detail) = failure {
            let (reason, race) = classify_simulation_failure(&detail);
            return Ok(AttemptReport {
                debt_mint: label,
                detail: Some(detail),
                position: market_key,
                race,
                // Nothing accrued in this lane is the ordinary answer, not an
                // alert, so an unrecognized refusal here reads as bounds.
                reason: if reason == ReasonCode::SimulationRejected {
                    ReasonCode::BoundsNotMet
                } else {
                    reason
                },
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let after = accounts
            .first()
            .and_then(Option::as_ref)
            .and_then(|data| token_amount(data))
            .unwrap_or(before);
        let paid = before.saturating_sub(after);
        if paid == 0 {
            return Ok(AttemptReport {
                debt_mint: label,
                detail: Some("settlement would move nothing".into()),
                position: market_key,
                race: None,
                reason: ReasonCode::AlreadyResolved,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        if self.dry_run {
            return Ok(AttemptReport {
                debt_mint: label,
                detail: Some(format!(
                    "would settle for {paid}; shadow mode does not send"
                )),
                position: market_key,
                race: None,
                reason: ReasonCode::ShadowMode,
                signature: None,
                status: OutcomeStatus::Skipped,
            });
        }

        let ceiling =
            (paid as u128).saturating_mul((10_000 + self.policy.slippage_bps) as u128) / 10_000;
        if crate::runtime::devnet::enabled() {
            let maximum = crate::liquidation::required_u64("KEEPER_AUCTION_MAX_PAYMENT")
                .map_err(ExecutionError::Assembly)?;
            if ceiling > u128::from(maximum) {
                return Ok(AttemptReport {
                    debt_mint: label,
                    detail: Some("auction exceeds devnet payment cap".into()),
                    position: market_key,
                    race: None,
                    reason: ReasonCode::BoundsNotMet,
                    signature: None,
                    status: OutcomeStatus::Skipped,
                });
            }
        }
        let (sending, _) = self.settle_instruction(
            market,
            futarchy,
            lane,
            source,
            sold_is_base,
            u64::try_from(ceiling).unwrap_or(u64::MAX),
        )?;
        let submitted = (|| -> Result<Option<String>, String> {
            let mut keys = sending
                .accounts
                .iter()
                .map(|account| account.pubkey)
                .collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            if let Ok(table) = env::var("KEEPER_LIQUIDATION_LOOKUP_TABLE") {
                keys.push(key(&table)?);
                keys.sort();
                keys.dedup();
            }
            let snapshot = Snapshot::fetch(self.client, &keys, 0).map_err(|e| e.to_string())?;
            let plan = vec![sending];
            let simulated = simulation::simulate_for(
                self.client,
                self.signer.public_key(),
                &snapshot,
                &plan,
                &[],
            )?;
            if !simulated["value"]["err"].is_null() {
                return Err("bounded auction simulation rejected".into());
            }
            let fresh =
                Snapshot::fetch(self.client, &keys, snapshot.slot).map_err(|e| e.to_string())?;
            let fingerprint = state_hash(&fresh, 0)?;
            if fingerprint != state_hash(&snapshot, 0)? {
                return Err("auction state changed; requote".into());
            }
            let checked = simulation::simulate_for(
                self.client,
                self.signer.public_key(),
                &fresh,
                &plan,
                &[],
            )?;
            if !checked["value"]["err"].is_null() {
                return Err("auction pre-submit simulation rejected".into());
            }
            let block: serde_json::Value = self
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
                self.signer.public_key(),
                &fresh,
                &plan,
                key(hash)?,
            )?;
            if serialize_transaction(&[[0; 64]], &message).len() > 1232 {
                return Err("auction transaction exceeds packet limit".into());
            }
            let mut journal = Journal::connect(
                &env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required before signing")?,
            )?;
            journal.submit(
                self.client,
                self.signer,
                &self.contract.protocol_revision,
                &address(market.address),
                u8::from(!sold_is_base),
                &address(market.address),
                &format!("auction:{label}"),
                fresh.slot,
                &fingerprint,
                &message,
                hash,
                height,
            )
        })();
        match submitted {
            Ok(signature) => Ok(AttemptReport {
                debt_mint: label,
                detail: Some(format!(
                    "auction journaled, paying at most {ceiling}; awaiting finalized reconciliation"
                )),
                position: market_key,
                race: None,
                reason: if signature.is_some() {
                    ReasonCode::Confirmed
                } else {
                    ReasonCode::StateChanged
                },
                status: if signature.is_some() {
                    OutcomeStatus::Executed
                } else {
                    OutcomeStatus::Skipped
                },
                signature,
            }),
            Err(error) => {
                let detail = error.to_string();
                let (reason, race) = classify_simulation_failure(&detail);
                Ok(AttemptReport {
                    debt_mint: label,
                    detail: Some(detail),
                    position: market_key,
                    race,
                    reason,
                    signature: None,
                    status: OutcomeStatus::RetryableFailure,
                })
            }
        }
    }

    pub fn run_pass(&self) -> Result<Vec<AttemptReport>, ExecutionError> {
        let futarchy = self.futarchy()?;
        let mut reports =
            recover_pending(self.client, &self.contract.protocol_revision, self.dry_run)?;
        let mut sent = 0_usize;

        for market in self.markets()? {
            for lane in [ProtocolAuctionLane::Fee, ProtocolAuctionLane::Buyback] {
                for source in [ProtocolRevenueSource::Swap, ProtocolRevenueSource::Interest] {
                    for sold_is_base in [true, false] {
                        if sent >= self.max_sends_per_pass {
                            return Ok(reports);
                        }
                        let report =
                            self.attempt(&market, &futarchy, lane, source, sold_is_base)?;
                        // A submission failure can mean an unknown landed transaction.
                        // Reserve the pass budget for it until journal reconciliation.
                        if matches!(
                            report.status,
                            OutcomeStatus::Executed | OutcomeStatus::RetryableFailure
                        ) || report.reason == ReasonCode::StateChanged
                        {
                            sent += 1;
                        }
                        reports.push(report);
                    }
                }
            }
        }
        Ok(reports)
    }
}

/// How much revenue the arbitrageur will buy, and at what tolerance.
///
/// Required rather than defaulted, like the other spending profiles: the
/// ceiling is the operator's money, and a keeper that picks it is picking how
/// much of someone else's balance to commit.
fn policy() -> Option<ArbitragePolicy> {
    Some(ArbitragePolicy {
        max_sold: env::var("KEEPER_AUCTION_MAX_SOLD").ok()?.parse().ok()?,
        slippage_bps: env::var("KEEPER_AUCTION_SLIPPAGE_BPS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(50),
    })
}

pub fn run(context: &super::WorkerContext<'_>) -> Result<Vec<AttemptReport>, ExecutionError> {
    let Some(policy) = policy() else {
        return Ok(Vec::new());
    };
    ArbitrageurJob {
        client: context.client,
        contract: context.contract,
        dry_run: context.dry_run,
        layout: context.layout,
        max_sends_per_pass: context.max_sends,
        policy,
        program_id: context.program_id,
        resolver: context.resolver,
        signer: context.signer,
    }
    .run_pass()
}
