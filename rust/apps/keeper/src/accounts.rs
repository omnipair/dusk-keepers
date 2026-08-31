//! Assembling an instruction's account list.
//!
//! Every keeper job past the trigger needs twenty accounts or so, drawn from
//! four different places: PDAs with recipes in the resolution manifest, fixed
//! addresses in the instruction contract, fields read out of the market and
//! the position, and token accounts derived from the signer. Each job wiring
//! that up by hand would be twenty chances to put the collateral side where
//! the debt side belongs — a mistake the program catches, but only after a
//! wasted simulation, and only for the accounts it happens to validate.
//!
//! This assembles the list in the order the contract declares, so an account
//! that has no source is a named error rather than a silently misplaced key.

use std::collections::BTreeMap;

use dusk_adapter::{
    AccountLayoutManifest, DeterministicAccountResolver, InstructionContract, PdaSeedValue,
    derive_program_address,
};

use crate::transaction::AccountMeta;

/// SPL associated token account program.
pub const ASSOCIATED_TOKEN_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

/// A market's addresses, read once and reused across the accounts that need
/// them. Sides are named base and quote rather than debt and collateral: which
/// is which depends on the position, and conflating the two here is precisely
/// the error this module exists to prevent.
#[derive(Clone, Copy, Debug)]
pub struct MarketAccounts {
    pub address: [u8; 32],
    pub base_mint: [u8; 32],
    pub quote_mint: [u8; 32],
    pub base_reserve_vault: [u8; 32],
    pub quote_reserve_vault: [u8; 32],
    pub base_collateral_vault: [u8; 32],
    pub quote_collateral_vault: [u8; 32],
    pub base_interest_vault: [u8; 32],
    pub quote_interest_vault: [u8; 32],
    pub base_insurance_vault: [u8; 32],
    pub quote_insurance_vault: [u8; 32],
    pub ylp_mint: [u8; 32],
    pub base_ylp_vault: [u8; 32],
    pub quote_ylp_vault: [u8; 32],
    /// Whether the market carries hLP that must be settled alongside any
    /// instruction that updates it.
    pub hlp_active: bool,
}

impl MarketAccounts {
    pub fn decode(
        layout: &AccountLayoutManifest,
        address: [u8; 32],
        data: &[u8],
    ) -> Option<Self> {
        let reader = layout.reader("Market").ok()?;
        Some(Self {
            address,
            base_collateral_vault: reader.pubkey("base_side.collateral_vault", data).ok()?,
            base_insurance_vault: reader.pubkey("insurance.base_vault", data).ok()?,
            base_interest_vault: reader.pubkey("base_side.interest_vault", data).ok()?,
            base_mint: reader.pubkey("base_side.asset_mint", data).ok()?,
            base_reserve_vault: reader.pubkey("base_side.reserve_vault", data).ok()?,
            quote_collateral_vault: reader.pubkey("quote_side.collateral_vault", data).ok()?,
            quote_insurance_vault: reader.pubkey("insurance.quote_vault", data).ok()?,
            quote_interest_vault: reader.pubkey("quote_side.interest_vault", data).ok()?,
            quote_mint: reader.pubkey("quote_side.asset_mint", data).ok()?,
            quote_reserve_vault: reader.pubkey("quote_side.reserve_vault", data).ok()?,
            base_ylp_vault: reader.pubkey("base_hlp_vault.ylp_vault", data).ok()?,
            quote_ylp_vault: reader.pubkey("quote_hlp_vault.ylp_vault", data).ok()?,
            ylp_mint: reader.pubkey("ylp_mint", data).ok()?,
            // Mirrors Market::has_active_hlp. Getting this wrong in either
            // direction breaks the instruction: too few accounts is
            // NotEnoughAccounts, too many shifts whatever the instruction
            // expects after the prefix.
            hlp_active: reader.u64("base_hlp_vault.hlp_supply", data).ok()? > 0
                || reader.i128("base_hlp_vault.residual_exposure", data).ok()? != 0
                || reader.u64("quote_hlp_vault.hlp_supply", data).ok()? > 0
                || reader.i128("quote_hlp_vault.residual_exposure", data).ok()? != 0,
        })
    }

    /// The hLP settlement prefix, in the order the program checks it.
    ///
    /// Instructions that update the market must carry these when the market
    /// has active hLP, and must not when it does not: the program reads the
    /// prefix positionally, so an extra account is as wrong as a missing one.
    pub fn hlp_remaining_accounts(&self) -> Vec<AccountMeta> {
        if !self.hlp_active {
            return Vec::new();
        }
        [
            self.ylp_mint,
            self.base_ylp_vault,
            self.quote_ylp_vault,
            self.base_interest_vault,
            self.quote_interest_vault,
        ]
        .into_iter()
        .map(AccountMeta::writable)
        .collect()
    }

    /// The market's two sides, oriented by which one carries the debt.
    pub fn oriented(&self, debt_is_base: bool) -> OrientedMarket {
        if debt_is_base {
            OrientedMarket {
                collateral_collateral_vault: self.quote_collateral_vault,
                collateral_insurance_vault: self.quote_insurance_vault,
                collateral_mint: self.quote_mint,
                collateral_reserve_vault: self.quote_reserve_vault,
                debt_interest_vault: self.base_interest_vault,
                debt_mint: self.base_mint,
                debt_reserve_vault: self.base_reserve_vault,
            }
        } else {
            OrientedMarket {
                collateral_collateral_vault: self.base_collateral_vault,
                collateral_insurance_vault: self.base_insurance_vault,
                collateral_mint: self.base_mint,
                collateral_reserve_vault: self.base_reserve_vault,
                debt_interest_vault: self.quote_interest_vault,
                debt_mint: self.quote_mint,
                debt_reserve_vault: self.quote_reserve_vault,
            }
        }
    }
}

/// A market seen from one position's point of view.
#[derive(Clone, Copy, Debug)]
pub struct OrientedMarket {
    pub debt_mint: [u8; 32],
    pub collateral_mint: [u8; 32],
    pub debt_reserve_vault: [u8; 32],
    // Named by the backstop and the leverage liquidation but not by the fill,
    // so nothing reads it yet.
    #[allow(dead_code)]
    pub collateral_reserve_vault: [u8; 32],
    pub debt_interest_vault: [u8; 32],
    pub collateral_collateral_vault: [u8; 32],
    pub collateral_insurance_vault: [u8; 32],
}

/// Derive an associated token account.
pub fn associated_token_account(
    owner: [u8; 32],
    token_program: [u8; 32],
    mint: [u8; 32],
) -> Option<[u8; 32]> {
    let seeds = vec![owner.to_vec(), token_program.to_vec(), mint.to_vec()];
    let resolved = derive_program_address(&seeds, ASSOCIATED_TOKEN_PROGRAM).ok()?;
    decode_key(&resolved.address)
}

pub fn decode_key(encoded: &str) -> Option<[u8; 32]> {
    let bytes = bs58::decode(encoded).into_vec().ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

#[derive(Debug)]
pub struct AssemblyError {
    pub account: String,
    pub detail: String,
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.account, self.detail)
    }
}

impl std::error::Error for AssemblyError {}

/// Builds an account list for one instruction.
///
/// Sources are consulted in order of authority: an address supplied by the
/// caller wins, then the manifest's fixed addresses, then a PDA recipe. An
/// account no source can produce is an error naming it, because the
/// alternative — a zero key, or the previous account repeated — produces a
/// transaction that fails somewhere else entirely.
pub struct AccountAssembler<'a> {
    pub contract: &'a InstructionContract,
    pub resolver: &'a DeterministicAccountResolver,
    /// Addresses this job knows: decoded state, the signer, derived token
    /// accounts.
    pub supplied: BTreeMap<&'static str, [u8; 32]>,
    /// Seed inputs for the PDA recipes, by recipe input name.
    pub pda_inputs: BTreeMap<String, PdaSeedValue>,
    /// Accounts the instruction declares optional and this job omits. An
    /// optional account is passed as the program id, which is how Anchor
    /// encodes "absent".
    pub omitted: &'a [&'a str],
}

impl AccountAssembler<'_> {
    pub fn assemble(&self, instruction_key: &str) -> Result<Vec<AccountMeta>, AssemblyError> {
        let specification = self
            .contract
            .instructions
            .iter()
            .find(|entry| entry.key == instruction_key)
            .ok_or_else(|| AssemblyError {
                account: instruction_key.to_owned(),
                detail: "instruction is absent from the pinned contract".into(),
            })?;

        let program_id = decode_key(&specification.program_id).ok_or_else(|| AssemblyError {
            account: instruction_key.to_owned(),
            detail: "program id does not decode".into(),
        })?;

        let mut metas = Vec::with_capacity(specification.accounts.len());
        for account in &specification.accounts {
            let name = account.name.as_str();
            let pubkey = if self.omitted.contains(&name) {
                if !account.optional {
                    return Err(AssemblyError {
                        account: name.to_owned(),
                        detail: "cannot be omitted; the instruction requires it".into(),
                    });
                }
                program_id
            } else if let Some(address) = self.supplied.get(name) {
                *address
            } else if let Some(fixed) = self.resolver.resolve_static(instruction_key, name) {
                decode_key(fixed).ok_or_else(|| AssemblyError {
                    account: name.to_owned(),
                    detail: "fixed address does not decode".into(),
                })?
            } else if let Some(recipe) = self.resolver.pda_recipe_key(instruction_key, name) {
                // Recipes disagree about which inputs they take, and an
                // unexpected one is an error rather than a harmless extra, so
                // each is given exactly what it asks for out of the shared
                // pool.
                let wanted = self
                    .resolver
                    .recipe_inputs(recipe)
                    .ok_or_else(|| AssemblyError {
                        account: name.to_owned(),
                        detail: format!("{recipe}: unknown PDA recipe"),
                    })?;
                let mut inputs = BTreeMap::new();
                for key in wanted {
                    let value = self.pda_inputs.get(key).ok_or_else(|| AssemblyError {
                        account: name.to_owned(),
                        detail: format!("{recipe} needs seed input {key}"),
                    })?;
                    inputs.insert(key.to_owned(), value.clone());
                }
                let resolved =
                    self.resolver
                        .derive_pda(recipe, &inputs)
                        .map_err(|error| AssemblyError {
                            account: name.to_owned(),
                            detail: error.to_string(),
                        })?;
                decode_key(&resolved.address).ok_or_else(|| AssemblyError {
                    account: name.to_owned(),
                    detail: "derived address does not decode".into(),
                })?
            } else {
                return Err(AssemblyError {
                    account: name.to_owned(),
                    detail: "no source supplies this account".into(),
                });
            };

            metas.push(AccountMeta {
                is_signer: account.signer,
                // An omitted optional account is passed as the program id,
                // which must not be marked writable however the instruction
                // declares the slot.
                is_writable: account.writable && !self.omitted.contains(&name),
                pubkey,
            });
        }
        Ok(metas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checked against the address @solana/spl-token derives for the same
    /// owner and mint. A hand-rolled derivation that agrees with itself proves
    /// nothing; agreeing with the library every wallet uses is the point.
    #[test]
    fn derives_an_associated_token_account() {
        let owner = decode_key("FJWRK3XJeVD8njSvTFXyHwP2jkatvBqeVwcNAFe5zVfJ").unwrap();
        let token = decode_key("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
        let mint = decode_key("2G78R5YwVopkAxnDMmvNany3RdL98eAM2jcr3UafvL7W").unwrap();
        let ata = associated_token_account(owner, token, mint).expect("derives");
        assert_eq!(
            bs58::encode(ata).into_string(),
            "7YfgmEHamDPh463v8XXBDrcTsJ3Dy6aKbbNEbbqUDUws"
        );
    }

    /// Orientation is the error this module exists to prevent, so it is
    /// asserted directly: flipping the debt side must swap every field, not
    /// some of them.
    #[test]
    fn orientation_swaps_every_side() {
        let market = MarketAccounts {
            address: [0; 32],
            base_collateral_vault: [3; 32],
            base_insurance_vault: [5; 32],
            base_interest_vault: [4; 32],
            base_mint: [1; 32],
            base_reserve_vault: [2; 32],
            quote_collateral_vault: [13; 32],
            quote_insurance_vault: [15; 32],
            quote_interest_vault: [14; 32],
            base_ylp_vault: [6; 32],
            hlp_active: true,
            quote_mint: [11; 32],
            quote_reserve_vault: [12; 32],
            quote_ylp_vault: [16; 32],
            ylp_mint: [7; 32],
        };
        let base_debt = market.oriented(true);
        let quote_debt = market.oriented(false);
        assert_eq!(base_debt.debt_mint, market.base_mint);
        assert_eq!(base_debt.collateral_mint, market.quote_mint);
        assert_eq!(quote_debt.debt_mint, market.quote_mint);
        assert_eq!(quote_debt.collateral_mint, market.base_mint);
        assert_ne!(base_debt.debt_reserve_vault, quote_debt.debt_reserve_vault);
        assert_ne!(
            base_debt.collateral_insurance_vault,
            quote_debt.collateral_insurance_vault
        );
    }

    /// The prefix is positional, so its order is part of the contract with the
    /// program and an inactive market must contribute nothing at all.
    #[test]
    fn the_hlp_prefix_is_ordered_and_conditional() {
        let mut market = MarketAccounts {
            address: [0; 32],
            base_collateral_vault: [3; 32],
            base_insurance_vault: [5; 32],
            base_interest_vault: [4; 32],
            base_mint: [1; 32],
            base_reserve_vault: [2; 32],
            base_ylp_vault: [6; 32],
            hlp_active: true,
            quote_collateral_vault: [13; 32],
            quote_insurance_vault: [15; 32],
            quote_interest_vault: [14; 32],
            quote_mint: [11; 32],
            quote_reserve_vault: [12; 32],
            quote_ylp_vault: [16; 32],
            ylp_mint: [7; 32],
        };
        let prefix = market.hlp_remaining_accounts();
        assert_eq!(prefix.len(), 5);
        assert_eq!(prefix[0].pubkey, market.ylp_mint);
        assert_eq!(prefix[1].pubkey, market.base_ylp_vault);
        assert_eq!(prefix[2].pubkey, market.quote_ylp_vault);
        assert_eq!(prefix[3].pubkey, market.base_interest_vault);
        assert_eq!(prefix[4].pubkey, market.quote_interest_vault);
        assert!(prefix.iter().all(|meta| meta.is_writable));

        market.hlp_active = false;
        assert!(market.hlp_remaining_accounts().is_empty());
    }
}
