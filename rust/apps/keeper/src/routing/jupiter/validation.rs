use super::{ApiInstruction, BuildResponse, PROGRAM, Result};
use crate::{
    liquidation::plan::{address, key},
    runtime::{
        accounts::{ASSOCIATED_TOKEN_PROGRAM, associated_token_account},
        rpc::decode_base64,
        transaction::{AccountMeta, Instruction},
    },
};
use dusk_adapter::transfer_fees::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use std::{collections::BTreeSet, time::Instant};

pub struct Request {
    pub buyer: [u8; 32],
    pub input_mint: [u8; 32],
    pub output_mint: [u8; 32],
    pub source: [u8; 32],
    pub destination: [u8; 32],
    pub input_program: [u8; 32],
    pub output_program: [u8; 32],
    pub amount: u64,
    // No Dusk custody, session, position or other protected writable accounts
    // may be delegated to the external route.
    pub protected: BTreeSet<[u8; 32]>,
}
pub struct ValidatedRoute {
    pub setup: Vec<Instruction>,
    pub swap: Instruction,
    pub tables: Vec<[u8; 32]>,
    pub minimum_output: u64,
    pub expires_at: Instant,
}
impl ValidatedRoute {
    pub fn read_keys(&self) -> Vec<[u8; 32]> {
        self.setup
            .iter()
            .chain([&self.swap])
            .flat_map(|i| std::iter::once(i.program_id).chain(i.accounts.iter().map(|a| a.pubkey)))
            .chain(self.tables.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}
impl ApiInstruction {
    fn decode(self) -> Result<Instruction> {
        if self.accounts.len() > 64 || self.data.len() > 2048 {
            return Err("oversized Jupiter instruction".into());
        }
        Ok(Instruction {
            program_id: key(&self.program_id)?,
            accounts: self
                .accounts
                .into_iter()
                .map(|a| {
                    Ok(AccountMeta {
                        pubkey: key(&a.pubkey)?,
                        is_signer: a.is_signer,
                        is_writable: a.is_writable,
                    })
                })
                .collect::<Result<_>>()?,
            data: decode_base64(&self.data).ok_or("invalid Jupiter instruction encoding")?,
        })
    }
}
fn atoms(s: &str) -> Result<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid Jupiter token amount".into());
    }
    s.parse()
        .map_err(|_| "Jupiter token amount overflow".into())
}
fn account(
    ix: &Instruction,
    index: usize,
    expected: [u8; 32],
    signer: bool,
    writable: bool,
) -> Result<()> {
    let a = ix.accounts.get(index).ok_or("truncated Jupiter accounts")?;
    if a.pubkey != expected || a.is_signer != signer || a.is_writable != writable {
        return Err("Jupiter account identity/privilege mismatch".into());
    }
    Ok(())
}
pub(super) fn validate(
    response: BuildResponse,
    r: &Request,
    slippage: u16,
) -> Result<ValidatedRoute> {
    if response.transaction_version != 0
        || response.swap_mode != "ExactIn"
        || response.input_mint != address(r.input_mint)
        || response.output_mint != address(r.output_mint)
        || atoms(&response.in_amount)? != r.amount
        || r.amount == 0
        || response.slippage_bps != slippage
        || slippage > 500
        || response.cleanup_instruction.is_some()
        || response.tip_instruction.is_some()
        || !response.other_instructions.is_empty()
        || response.setup_instructions.len() > 4
    {
        return Err("Jupiter build does not match the permitted exact-in swap".into());
    }
    if associated_token_account(r.buyer, r.input_program, r.input_mint) != Some(r.source)
        || associated_token_account(r.buyer, r.output_program, r.output_mint) != Some(r.destination)
    {
        return Err("Jupiter requires the keeper's canonical token accounts".into());
    }
    let program = key(PROGRAM)?;
    let swap = response.swap_instruction.decode()?;
    // Financial fields precede the opaque, program-validated route DAG. Layout
    // is pinned in aggregator-idl.json, captured from Jupiter's on-chain IDL.
    // Unknown/legacy/shared/ledger variants fail closed rather than guessing.
    if swap.program_id != program
        || swap.data.len() < 39
        || swap.data[..8] != [187, 100, 250, 204, 49, 196, 175, 20]
        || swap.data[8..16] != r.amount.to_le_bytes()
        || swap.data[16..24] != atoms(&response.out_amount)?.to_le_bytes()
        || swap.data[24..26] != slippage.to_le_bytes()
        || swap.data[26..30] != [0; 4]
    {
        return Err("unsupported Jupiter instruction or mismatched on-chain swap bounds".into());
    }
    let steps = u32::from_le_bytes(swap.data[30..34].try_into().unwrap());
    if !(1..=8).contains(&steps) {
        return Err("Jupiter route complexity exceeds limit".into());
    }
    account(&swap, 0, r.buyer, true, false)?;
    account(&swap, 1, r.source, false, true)?;
    account(&swap, 2, r.destination, false, true)?;
    account(&swap, 3, r.input_mint, false, false)?;
    account(&swap, 4, r.output_mint, false, false)?;
    account(&swap, 5, r.input_program, false, false)?;
    account(&swap, 6, r.output_program, false, false)?;
    // Explicit destination may be repeated here, or absent via Anchor sentinel.
    if swap.accounts.get(7).is_some_and(|a| a.pubkey == program) {
        account(&swap, 7, program, false, false)?;
    } else {
        account(&swap, 7, r.destination, false, true)?;
    }
    account(
        &swap,
        8,
        key("D8cy77BBepLMngZx6ZukaTff5hCt1HrWyKk3Hnd9oitf")?,
        false,
        false,
    )?;
    account(&swap, 9, program, false, false)?;
    for a in &swap.accounts {
        if (a.is_signer && a.pubkey != r.buyer)
            || (a.is_writable && r.protected.contains(&a.pubkey))
            // Do not delegate SOL spending to a route CPI. Rent is limited to
            // the explicitly validated idempotent ATA creations below.
            || (a.is_writable && a.pubkey == r.buyer)
        {
            return Err("Jupiter route requests forbidden account privileges".into());
        }
    }
    // Floor is deliberately conservative if Jupiter rounds its displayed
    // threshold upward. We verify binary fields, not merely quote JSON.
    let minimum_output =
        (u128::from(atoms(&response.out_amount)?) * u128::from(10_000 - slippage) / 10_000) as u64;
    let reported = atoms(&response.other_amount_threshold)?;
    if reported < minimum_output || reported > minimum_output.saturating_add(1) {
        return Err("Jupiter threshold disagrees with the on-chain bounds".into());
    }
    let mut setup = Vec::new();
    for instruction in response.setup_instructions {
        let ix = instruction.decode()?;
        if ix.program_id != key(ASSOCIATED_TOKEN_PROGRAM)?
            || ix.data != [1]
            || ix.accounts.len() != 6
        {
            return Err("Jupiter setup must only create idempotent token accounts".into());
        }
        let mint = ix.accounts[3].pubkey;
        let token = ix.accounts[5].pubkey;
        if token != key(TOKEN_PROGRAM)? && token != key(TOKEN_2022_PROGRAM)? {
            return Err("unsupported Jupiter token program".into());
        }
        let ata = associated_token_account(r.buyer, token, mint)
            .ok_or("Jupiter ATA derivation failed")?;
        account(&ix, 0, r.buyer, true, true)?;
        account(&ix, 1, ata, false, true)?;
        account(&ix, 2, r.buyer, false, false)?;
        account(&ix, 3, mint, false, false)?;
        account(&ix, 4, [0; 32], false, false)?;
        account(&ix, 5, token, false, false)?;
        if r.protected.contains(&ata)
            || !swap
                .accounts
                .iter()
                .any(|a| a.pubkey == ata && a.is_writable)
        {
            return Err("unrelated Jupiter setup account".into());
        }
        setup.push(ix);
    }
    // Ignore the API's price recommendation, but reject unexpected programs or
    // opcodes. Keeper resources provide one limit and its own bounded price.
    for instruction in response.compute_budget_instructions {
        let ix = instruction.decode()?;
        if ix.program_id != key("ComputeBudget111111111111111111111111111111")?
            || !ix.accounts.is_empty()
            || ix.data.len() != 9
            || ix.data[0] != 3
        {
            return Err("unexpected Jupiter compute instruction".into());
        }
    }
    let tables = response
        .addresses_by_lookup_table_address
        .unwrap_or_default()
        .keys()
        .map(|s| key(s))
        .collect::<Result<Vec<_>>>()?;
    if tables.len() > 8 {
        return Err("too many Jupiter lookup tables".into());
    }
    Ok(ValidatedRoute {
        setup,
        swap,
        tables,
        minimum_output,
        expires_at: Instant::now(),
    })
}
