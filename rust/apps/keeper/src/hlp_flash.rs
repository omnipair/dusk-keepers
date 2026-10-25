//! Vault-wide hLP flash recovery and terminal-close instruction assembly.

use crate::{
    accounts::{ASSOCIATED_TOKEN_PROGRAM, MarketAccounts, associated_token_account, decode_key},
    discovery::account_discriminator,
    flash::{InventoryPaymentAccounts, Snapshot, address, inventory_payment_for, key},
    transaction::{AccountMeta, Instruction},
};
use dusk_adapter::{
    AccountLayoutManifest, BeginHlpFlashArgs, CloseInsolventHlpArgs, InstructionContract,
    KeeperInstructionArguments as Args, PreviewHlpFlashArgs, derive_program_address,
    encode_keeper_instruction,
    liquidation::HlpFlashQuote,
    transfer_fees::{CLOCK, decode_clock_epoch, decode_mint_fees},
};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, String>;

fn pda(program: [u8; 32], seeds: &[&[u8]]) -> Result<[u8; 32]> {
    key(&derive_program_address(
        &seeds.iter().map(|seed| seed.to_vec()).collect::<Vec<_>>(),
        &address(program),
    )
    .map_err(|error| error.to_string())?
    .address)
}

#[derive(Clone, Debug)]
pub struct HlpContext {
    pub program: [u8; 32],
    pub buyer: [u8; 32],
    pub market: MarketAccounts,
    pub target_asset: u8,
    pub epoch: u64,
    pub named: BTreeMap<String, [u8; 32]>,
    pub collateral_token: [u8; 32],
    pub debt_token: [u8; 32],
}

impl HlpContext {
    pub fn resolve(
        layout: &AccountLayoutManifest,
        snapshot: &Snapshot,
        market_key: [u8; 32],
        buyer: [u8; 32],
        target_asset: u8,
    ) -> Result<Self> {
        if target_asset > 1 {
            return Err("invalid hLP target side".into());
        }
        let program = key(&layout.program_id)?;
        let market_data = snapshot.owned(market_key, program)?;
        if market_data.get(..8) != Some(account_discriminator("Market").as_slice()) {
            return Err("market discriminator".into());
        }
        let market =
            MarketAccounts::decode(layout, market_key, market_data).ok_or("market layout")?;
        let target_is_base = target_asset == 0;
        let target_mint = if target_is_base {
            market.base_mint
        } else {
            market.quote_mint
        };
        let borrowed_mint = if target_is_base {
            market.quote_mint
        } else {
            market.base_mint
        };
        let target_reserve = if target_is_base {
            market.base_reserve_vault
        } else {
            market.quote_reserve_vault
        };
        let borrowed_reserve = if target_is_base {
            market.quote_reserve_vault
        } else {
            market.base_reserve_vault
        };
        let insurance_vault = if target_is_base {
            market.quote_insurance_vault
        } else {
            market.base_insurance_vault
        };
        let borrowed_interest_vault = if target_is_base {
            market.quote_interest_vault
        } else {
            market.base_interest_vault
        };
        let target_hlp_ylp_vault = if target_is_base {
            market.base_ylp_vault
        } else {
            market.quote_ylp_vault
        };
        let collateral_mint_account = snapshot.account(target_mint)?;
        let debt_mint_account = snapshot.account(borrowed_mint)?;
        decode_mint_fees(
            &collateral_mint_account.owner,
            &collateral_mint_account.data,
        )?;
        decode_mint_fees(&debt_mint_account.owner, &debt_mint_account.data)?;
        let collateral_token = key(&collateral_mint_account.owner)?;
        let debt_token = key(&debt_mint_account.owner)?;
        let clock = snapshot.account(key(CLOCK)?)?;
        let epoch = decode_clock_epoch(&clock.owner, &clock.data, snapshot.slot)?;
        let session = pda(
            program,
            &[b"hlp-flash-session", &market_key, &[target_asset]],
        )?;
        let repayment_vault = pda(program, &[b"hlp-flash-payment", &session])?;
        let buyer_collateral_account =
            associated_token_account(buyer, collateral_token, target_mint)
                .ok_or("buyer collateral ATA")?;
        let buyer_refund_account =
            associated_token_account(buyer, debt_token, borrowed_mint).ok_or("buyer debt ATA")?;
        let named = BTreeMap::from([
            ("market".into(), market_key),
            (
                "futarchy_authority".into(),
                pda(program, &[b"futarchy_authority"])?,
            ),
            ("buyer".into(), buyer),
            ("caller".into(), buyer),
            ("target_mint".into(), target_mint),
            ("borrowed_mint".into(), borrowed_mint),
            ("target_reserve".into(), target_reserve),
            ("borrowed_reserve".into(), borrowed_reserve),
            ("borrowed_reserve_vault".into(), borrowed_reserve),
            ("borrowed_interest_vault".into(), borrowed_interest_vault),
            ("buyer_collateral_account".into(), buyer_collateral_account),
            ("buyer_refund_account".into(), buyer_refund_account),
            ("caller_bounty_account".into(), buyer_refund_account),
            ("insurance_vault".into(), insurance_vault),
            ("borrowed_token_program".into(), debt_token),
            ("session".into(), session),
            ("repayment_vault".into(), repayment_vault),
            ("ylp_mint".into(), market.ylp_mint),
            ("target_hlp_ylp_vault".into(), target_hlp_ylp_vault),
            (
                "event_authority".into(),
                pda(program, &[b"__event_authority"])?,
            ),
            ("program".into(), program),
        ]);
        Ok(Self {
            program,
            buyer,
            market,
            target_asset,
            epoch,
            named,
            collateral_token,
            debt_token,
        })
    }

    fn instruction(
        &self,
        contract: &InstructionContract,
        args: Args,
        include_hlp_accounts: bool,
    ) -> Result<Instruction> {
        let specification = contract
            .instructions
            .iter()
            .find(|entry| entry.key == args.specification_key())
            .ok_or("missing hLP instruction contract")?;
        let mut accounts = Vec::with_capacity(specification.accounts.len() + 5);
        for account in &specification.accounts {
            let address = self
                .named
                .get(&account.name)
                .copied()
                .or_else(|| account.fixed_address.as_deref().and_then(decode_key))
                .ok_or_else(|| format!("missing {}", account.name))?;
            accounts.push(AccountMeta {
                pubkey: address,
                is_signer: account.signer,
                is_writable: account.writable,
            });
        }
        if include_hlp_accounts {
            accounts.extend(self.market.hlp_remaining_accounts());
        }
        Ok(Instruction {
            program_id: self.program,
            accounts,
            data: encode_keeper_instruction(contract, &args).map_err(|error| error.to_string())?,
        })
    }

    pub fn setup(&self, snapshot: &Snapshot) -> Result<Vec<Instruction>> {
        let mut instructions = Vec::new();
        for (name, mint, token_program) in [
            (
                "buyer_collateral_account",
                self.named["target_mint"],
                self.collateral_token,
            ),
            (
                "buyer_refund_account",
                self.named["borrowed_mint"],
                self.debt_token,
            ),
        ] {
            let ata = self.named[name];
            if let Some(Some(account)) = snapshot.accounts.get(&address(ata)) {
                if account.owner != address(token_program)
                    || account.data.get(..32) != Some(mint.as_slice())
                    || account.data.get(32..64) != Some(self.buyer.as_slice())
                    || account.data.get(108) != Some(&1)
                {
                    return Err(format!("invalid {name}"));
                }
                continue;
            }
            instructions.push(Instruction {
                program_id: key(ASSOCIATED_TOKEN_PROGRAM)?,
                data: vec![1],
                accounts: vec![
                    AccountMeta::signer(self.buyer),
                    AccountMeta::writable(ata),
                    AccountMeta::readonly(self.buyer),
                    AccountMeta::readonly(mint),
                    AccountMeta::readonly([0; 32]),
                    AccountMeta::readonly(token_program),
                ],
            });
        }
        Ok(instructions)
    }

    pub fn preview(&self, contract: &InstructionContract) -> Result<Instruction> {
        self.instruction(
            contract,
            Args::PreviewHlpFlash(PreviewHlpFlashArgs {
                target_asset: self.target_asset,
            }),
            false,
        )
    }

    pub fn flash_plan(
        &self,
        contract: &InstructionContract,
        snapshot: &Snapshot,
        quote: &HlpFlashQuote,
        max_insurance_draw: u64,
        max_socialized_loss: u64,
    ) -> Result<(Vec<Instruction>, u64)> {
        let prefix = crate::flash::resources()?;
        let (payment, gross_payment) = inventory_payment_for(
            snapshot,
            InventoryPaymentAccounts {
                buyer: self.buyer,
                token_program: self.debt_token,
                mint: self.named["borrowed_mint"],
                source: self.named["buyer_refund_account"],
                destination: self.named["repayment_vault"],
            },
            self.epoch,
            quote.payment,
        )?;
        let settle_index = u16::try_from(prefix.len() + 2).map_err(|_| "settle index overflow")?;
        let begin = self.instruction(
            contract,
            Args::BeginHlpFlash(BeginHlpFlashArgs {
                target_asset: self.target_asset,
                max_payment: quote.payment,
                allow_full: quote.full,
                max_insurance_draw,
                max_socialized_loss,
                max_collateral_debit: quote.collateral_debit,
                min_collateral_credit: quote.collateral_credit,
                settle_index,
            }),
            false,
        )?;
        let settle = self.instruction(contract, Args::SettleHlpFlash, true)?;
        let mut instructions = prefix;
        instructions.push(begin);
        instructions.push(payment);
        instructions.push(settle);
        Ok((instructions, gross_payment))
    }

    pub fn close_insolvent(
        &self,
        contract: &InstructionContract,
        max_insurance_draw: u64,
        max_socialized_loss: u64,
        min_caller_bounty_out: u64,
    ) -> Result<Vec<Instruction>> {
        let mut instructions = crate::flash::resources()?;
        instructions.push(self.instruction(
            contract,
            Args::CloseInsolventHlp(CloseInsolventHlpArgs {
                target_asset: self.target_asset,
                max_insurance_draw,
                max_socialized_loss,
                min_caller_bounty_out,
            }),
            false,
        )?);
        Ok(instructions)
    }
}
