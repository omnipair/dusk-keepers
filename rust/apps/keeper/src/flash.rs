//! Atomic liquidation plans for the captured #45 interface. No auction calls.
use crate::{
    accounts::{ASSOCIATED_TOKEN_PROGRAM, MarketAccounts, associated_token_account, decode_key},
    discovery::{DiscoveryError, RpcClient, decode_base64},
    transaction::{AccountMeta, Instruction},
};
use borsh::BorshDeserialize;
use dusk_adapter::liquidation::{EmergencyLiquidationQuote, FlashLiquidationQuote};
use dusk_adapter::transfer_fees::{CLOCK, decode_clock_epoch, decode_mint_fees};
use dusk_adapter::{
    AccountLayoutManifest, BeginFlashLiquidationArgs, EmergencyLiquidationArgs,
    InstructionContract, KeeperInstructionArguments as Args, PreviewEmergencyLiquidationArgs,
    PreviewFlashLiquidationArgs, PreviewLiquidationHealthArgs, derive_program_address,
    encode_keeper_instruction,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, String>;
pub struct FlashPlanOptions {
    pub max_repayment: u64,
    pub prefix: Vec<Instruction>,
    pub route: Vec<Instruction>,
    pub internal: bool,
    pub min_buyer_credit: u64,
}
pub fn key(v: &str) -> Result<[u8; 32]> {
    decode_key(v).ok_or_else(|| "invalid public key".into())
}
pub fn address(v: [u8; 32]) -> String {
    bs58::encode(v).into_string()
}
fn pda(program: [u8; 32], seeds: &[&[u8]]) -> Result<[u8; 32]> {
    key(&derive_program_address(
        &seeds.iter().map(|s| s.to_vec()).collect::<Vec<_>>(),
        &address(program),
    )
    .map_err(|e| e.to_string())?
    .address)
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Account {
    pub owner: String,
    pub data: Vec<u8>,
    pub executable: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub slot: u64,
    pub accounts: BTreeMap<String, Option<Account>>,
}
impl Snapshot {
    pub fn account(&self, k: [u8; 32]) -> Result<&Account> {
        self.accounts
            .get(&address(k))
            .and_then(Option::as_ref)
            .filter(|a| !a.executable)
            .ok_or_else(|| format!("missing data account {}", address(k)))
    }
    pub fn owned(&self, k: [u8; 32], owner: [u8; 32]) -> Result<&[u8]> {
        let a = self.account(k)?;
        if a.owner != address(owner) {
            return Err("account owner mismatch".into());
        }
        Ok(&a.data)
    }
    pub fn fetch(
        client: &RpcClient,
        keys: &[[u8; 32]],
        min_slot: u64,
    ) -> std::result::Result<Self, DiscoveryError> {
        if keys.len() > 100 {
            return Err(DiscoveryError::Malformed(
                "snapshot exceeds RPC batch".into(),
            ));
        }
        let response:serde_json::Value=client.call("getMultipleAccounts",serde_json::json!([keys.iter().copied().map(address).collect::<Vec<_>>(),{"encoding":"base64","commitment":"confirmed","minContextSlot":min_slot}]))?;
        let slot = response["context"]["slot"]
            .as_u64()
            .ok_or_else(|| DiscoveryError::Malformed("snapshot slot".into()))?;
        let values = response["value"]
            .as_array()
            .ok_or_else(|| DiscoveryError::Malformed("snapshot accounts".into()))?;
        if slot < min_slot || values.len() != keys.len() {
            return Err(DiscoveryError::Malformed(
                "stale/incomplete snapshot".into(),
            ));
        }
        let mut accounts = BTreeMap::new();
        for (k, v) in keys.iter().zip(values) {
            let a = if v.is_null() {
                None
            } else {
                if v["data"][1] != "base64" {
                    return Err(DiscoveryError::Malformed("snapshot encoding".into()));
                }
                Some(Account {
                    owner: v["owner"]
                        .as_str()
                        .ok_or_else(|| DiscoveryError::Malformed("snapshot owner".into()))?
                        .into(),
                    executable: v["executable"]
                        .as_bool()
                        .ok_or_else(|| DiscoveryError::Malformed("snapshot executable".into()))?,
                    data: decode_base64(v["data"][0].as_str().unwrap_or(""))
                        .ok_or_else(|| DiscoveryError::Malformed("snapshot base64".into()))?,
                })
            };
            accounts.insert(address(*k), a);
        }
        Ok(Self { slot, accounts })
    }
}
#[derive(Clone, Debug)]
pub struct Context {
    pub program: [u8; 32],
    pub buyer: [u8; 32],
    pub position: [u8; 32],
    pub market: MarketAccounts,
    pub kind: String,
    pub debt_asset: u8,
    pub collateral: u64,
    pub shares: u128,
    pub epoch: u64,
    pub named: BTreeMap<String, [u8; 32]>,
    pub collateral_token: [u8; 32],
    pub debt_token: [u8; 32],
    pub owner: [u8; 32],
}
impl Context {
    pub fn resolve(
        layout: &AccountLayoutManifest,
        snapshot: &Snapshot,
        position: [u8; 32],
        buyer: [u8; 32],
        kind: &str,
        debt_asset: u8,
    ) -> Result<Self> {
        if debt_asset > 1 || !["BorrowPosition", "LeveragePosition"].contains(&kind) {
            return Err("invalid position kind or side".into());
        }
        let program = key(&layout.program_id)?;
        let data = snapshot.owned(position, program)?;
        if data.get(..8) != Some(crate::discovery::account_discriminator(kind).as_slice()) {
            return Err("position discriminator".into());
        }
        let reader = layout.reader(kind).map_err(|e| e.to_string())?;
        let read_key = |f| reader.pubkey(f, data).map_err(|e| e.to_string());
        let owner = read_key("owner")?;
        let market_key = read_key("market")?;
        let market_data = snapshot.owned(market_key, program)?;
        if market_data.get(..8)
            != Some(crate::discovery::account_discriminator("Market").as_slice())
        {
            return Err("market discriminator".into());
        }
        let market =
            MarketAccounts::decode(layout, market_key, market_data).ok_or("market layout")?;
        if read_key("active_liquidation_session")? != [0; 32] {
            return Err("position is locked".into());
        }
        let leverage = kind == "LeveragePosition";
        if leverage && reader.u8("debt_asset", data).map_err(|e| e.to_string())? != debt_asset {
            return Err("wrong leverage debt side".into());
        }
        let shares_path = if leverage {
            "debt_shares"
        } else if debt_asset == 0 {
            "fixed_base_shares"
        } else {
            "fixed_quote_shares"
        };
        let shares = u128::from_le_bytes(
            reader
                .bytes(shares_path, data)
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "shares length")?,
        );
        let sides = market.oriented(debt_asset == 0);
        let collateral = reader
            .u64(
                if leverage {
                    "collateral_amount"
                } else if debt_asset == 0 {
                    "quote_collateral"
                } else {
                    "base_collateral"
                },
                data,
            )
            .map_err(|e| e.to_string())?;
        let cm = snapshot.account(sides.collateral_mint)?;
        let dm = snapshot.account(sides.debt_mint)?;
        decode_mint_fees(&cm.owner, &cm.data)?;
        decode_mint_fees(&dm.owner, &dm.data)?;
        let clock = snapshot.account(key(CLOCK)?)?;
        let epoch = decode_clock_epoch(&clock.owner, &clock.data, snapshot.slot)?;
        let collateral_token = key(&cm.owner)?;
        let debt_token = key(&dm.owner)?;
        let partner = read_key(if leverage {
            "referral_partner"
        } else if debt_asset == 0 {
            "base_referral_partner"
        } else {
            "quote_referral_partner"
        })?;
        let mut named = BTreeMap::from([
            ("market".into(), market_key),
            ("buyer".into(), buyer),
            ("collateral_mint".into(), sides.collateral_mint),
            ("debt_mint".into(), sides.debt_mint),
            (
                if leverage {
                    "leverage_position".into()
                } else {
                    "borrow_position".into()
                },
                position,
            ),
            (
                "collateral_vault".into(),
                if leverage {
                    pda(
                        program,
                        &[b"leverage_collateral", &market_key, &sides.collateral_mint],
                    )?
                } else {
                    sides.collateral_collateral_vault
                },
            ),
            ("reserve_vault".into(), sides.debt_reserve_vault),
            ("interest_vault".into(), sides.debt_interest_vault),
            ("insurance_vault".into(), sides.debt_insurance_vault),
            (
                "buyer_collateral_account".into(),
                associated_token_account(buyer, collateral_token, sides.collateral_mint)
                    .ok_or("collateral ATA")?,
            ),
            (
                "buyer_refund_account".into(),
                associated_token_account(buyer, debt_token, sides.debt_mint).ok_or("debt ATA")?,
            ),
            (
                "owner_debt_account".into(),
                associated_token_account(owner, debt_token, sides.debt_mint).ok_or("owner ATA")?,
            ),
            ("debt_token_program".into(), debt_token),
            (
                "futarchy_authority".into(),
                pda(program, &[b"futarchy_authority"])?,
            ),
            (
                "session".into(),
                pda(program, &[b"liquidation-session", &position])?,
            ),
            (
                "repayment_vault".into(),
                pda(
                    program,
                    &[b"liquidation-payment", &position, &sides.debt_mint],
                )?,
            ),
            (
                "event_authority".into(),
                pda(program, &[b"__event_authority"])?,
            ),
            ("program".into(), program),
        ]);
        if partner != [0; 32] {
            named.insert("referral_partner".into(), partner);
            named.insert(
                "referral_accrual".into(),
                pda(
                    program,
                    &[b"referral_accrual", &partner, &market_key, &sides.debt_mint],
                )?,
            );
        }
        Ok(Self {
            program,
            buyer,
            position,
            market,
            kind: kind.into(),
            debt_asset,
            collateral,
            shares,
            epoch,
            named,
            collateral_token,
            debt_token,
            owner,
        })
    }
    pub fn instruction(
        &self,
        contract: &InstructionContract,
        args: Args,
        internal: bool,
    ) -> Result<Instruction> {
        let spec = contract
            .instructions
            .iter()
            .find(|s| s.key == args.specification_key())
            .ok_or("missing instruction contract")?;
        let mut named = self.named.clone();
        if internal {
            named.insert(
                "collateral_reserve_vault".into(),
                self.market
                    .oriented(self.debt_asset == 0)
                    .collateral_reserve_vault,
            );
        }
        let mut accounts = Vec::new();
        for a in &spec.accounts {
            let k = named
                .get(&a.name)
                .copied()
                .or_else(|| a.fixed_address.as_deref().and_then(decode_key));
            let absent = k.is_none() && a.optional;
            accounts.push(AccountMeta {
                pubkey: k
                    .or(if absent { Some(self.program) } else { None })
                    .ok_or_else(|| format!("missing {}", a.name))?,
                is_signer: a.signer && !absent,
                is_writable: a.writable && !absent,
            });
        }
        if internal {
            accounts.extend(self.market.hlp_remaining_accounts());
        }
        Ok(Instruction {
            program_id: self.program,
            accounts,
            data: encode_keeper_instruction(contract, &args).map_err(|e| e.to_string())?,
        })
    }
    pub fn setup(&self, snapshot: &Snapshot) -> Result<Vec<Instruction>> {
        let mut out = Vec::new();
        for (name, owner, mint, token) in [
            (
                "buyer_collateral_account",
                self.buyer,
                self.named["collateral_mint"],
                self.collateral_token,
            ),
            (
                "buyer_refund_account",
                self.buyer,
                self.named["debt_mint"],
                self.debt_token,
            ),
            (
                "owner_debt_account",
                self.owner,
                self.named["debt_mint"],
                self.debt_token,
            ),
        ] {
            let ata = self.named[name];
            if let Some(Some(a)) = snapshot.accounts.get(&address(ata)) {
                if a.owner != address(token)
                    || a.data.get(..32) != Some(mint.as_slice())
                    || a.data.get(32..64) != Some(owner.as_slice())
                    || a.data.get(108) != Some(&1)
                {
                    return Err(format!("invalid {name}"));
                }
                continue;
            }
            if out
                .iter()
                .any(|i: &Instruction| i.accounts[1].pubkey == ata)
            {
                continue;
            }
            out.push(Instruction {
                program_id: key(ASSOCIATED_TOKEN_PROGRAM)?,
                data: vec![1],
                accounts: vec![
                    AccountMeta::signer(self.buyer),
                    AccountMeta::writable(ata),
                    AccountMeta::readonly(owner),
                    AccountMeta::readonly(mint),
                    AccountMeta::readonly([0; 32]),
                    AccountMeta::readonly(token),
                ],
            });
        }
        Ok(out)
    }
    pub fn health_preview(&self, c: &InstructionContract) -> Result<Instruction> {
        self.instruction(
            c,
            Args::PreviewLiquidationHealth(PreviewLiquidationHealthArgs {
                debt_asset: self.debt_asset,
            }),
            false,
        )
    }
    pub fn preview_flash(
        &self,
        c: &InstructionContract,
        max_repayment: u64,
        full: bool,
    ) -> Result<Instruction> {
        self.instruction(
            c,
            Args::PreviewFlashLiquidation(PreviewFlashLiquidationArgs {
                debt_asset: self.debt_asset,
                max_repayment,
                full,
            }),
            false,
        )
    }
    pub fn preview_emergency(
        &self,
        c: &InstructionContract,
        collateral_debit: u64,
        full: bool,
    ) -> Result<Instruction> {
        self.instruction(
            c,
            Args::PreviewEmergencyLiquidation(PreviewEmergencyLiquidationArgs {
                debt_asset: self.debt_asset,
                collateral_debit,
                full,
            }),
            false,
        )
    }
    pub fn flash(
        &self,
        c: &InstructionContract,
        q: &FlashLiquidationQuote,
        options: FlashPlanOptions,
    ) -> Result<Vec<Instruction>> {
        let FlashPlanOptions {
            max_repayment,
            prefix,
            route,
            internal,
            min_buyer_credit,
        } = options;
        if internal && !route.is_empty() {
            return Err("internal settlement cannot carry an external route".into());
        }
        let settle_index =
            u16::try_from(prefix.len() + 1 + route.len()).map_err(|_| "settle index overflow")?;
        let begin = self.instruction(
            c,
            Args::BeginFlashLiquidation(BeginFlashLiquidationArgs {
                position: self.position,
                debt_asset: self.debt_asset,
                max_repayment,
                max_payment: q.payment,
                min_collateral_credit: q.collateral_credit,
                min_buyer_credit: if internal { min_buyer_credit } else { 0 },
                full: q.full,
                settle_index,
            }),
            false,
        )?;
        let settle = self.instruction(c, Args::SettleFlashLiquidation, internal)?;
        let mut out = prefix;
        out.push(begin);
        out.extend(route);
        out.push(settle);
        Ok(out)
    }
    pub fn emergency(
        &self,
        c: &InstructionContract,
        q: &EmergencyLiquidationQuote,
    ) -> Result<Instruction> {
        self.instruction(
            c,
            Args::EmergencyLiquidation(EmergencyLiquidationArgs {
                debt_asset: self.debt_asset,
                collateral_debit: q.collateral_debit,
                full: q.full,
                min_reward_credit: q.reward_credit,
                min_swap_output: q.swap_output,
            }),
            true,
        )
    }
}
pub fn resources() -> Result<Vec<Instruction>> {
    let program_id = key("ComputeBudget111111111111111111111111111111")?;
    Ok([(1u8, 262144u32), (2, 1_400_000)]
        .into_iter()
        .map(|(op, v)| Instruction {
            program_id,
            accounts: vec![],
            data: [vec![op], v.to_le_bytes().to_vec()].concat(),
        })
        .collect())
}
pub fn decode_return<T: BorshDeserialize>(v: &serde_json::Value, program: [u8; 32]) -> Result<T> {
    if v["programId"] != address(program) || v["data"][1] != "base64" {
        return Err("preview return provenance".into());
    }
    let bytes = decode_base64(v["data"][0].as_str().ok_or("preview return encoding")?)
        .ok_or("preview base64")?;
    T::try_from_slice(&bytes).map_err(|e| format!("preview return layout: {e}"))
}
pub fn token_amount(data: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(64..72)?.try_into().ok()?))
}

/// Inventory-funded external settlement: the keeper buys collateral with its
/// own debt tokens. The required escrow credit is grossed up at the live fee.
pub fn inventory_payment(c: &Context, s: &Snapshot, net: u64) -> Result<(Instruction, u64)> {
    inventory_payment_for(
        s,
        InventoryPaymentAccounts {
            buyer: c.buyer,
            token_program: c.debt_token,
            mint: c.named["debt_mint"],
            source: c.named["buyer_refund_account"],
            destination: c.named["repayment_vault"],
        },
        c.epoch,
        net,
    )
}

#[derive(Clone, Copy, Debug)]
pub struct InventoryPaymentAccounts {
    pub buyer: [u8; 32],
    pub token_program: [u8; 32],
    pub mint: [u8; 32],
    pub source: [u8; 32],
    pub destination: [u8; 32],
}

pub fn inventory_payment_for(
    s: &Snapshot,
    accounts: InventoryPaymentAccounts,
    epoch: u64,
    net: u64,
) -> Result<(Instruction, u64)> {
    let mint = s.account(accounts.mint)?;
    let fee = decode_mint_fees(&mint.owner, &mint.data)?;
    let credit = |gross| fee.map_or(gross, |f| f.credit(epoch, gross, false, false));
    if credit(u64::MAX) < net {
        return Err("payment cannot be grossed up".into());
    }
    let (mut low, mut high) = (net, u64::MAX);
    while low < high {
        let mid = low + (high - low) / 2;
        if credit(mid) >= net {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    let mut data = vec![12];
    data.extend(low.to_le_bytes());
    data.push(*mint.data.get(44).ok_or("mint decimals")?);
    Ok((
        Instruction {
            program_id: accounts.token_program,
            accounts: vec![
                AccountMeta::writable(accounts.source),
                AccountMeta::readonly(accounts.mint),
                AccountMeta::writable(accounts.destination),
                AccountMeta::signer(accounts.buyer),
            ],
            data,
        },
        low,
    ))
}
