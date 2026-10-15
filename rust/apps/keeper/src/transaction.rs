//! Legacy transaction assembly.
//!
//! Written out rather than taken from `solana-sdk` because that crate pulls in
//! a very large dependency tree for what a keeper actually needs: build a
//! message, sign it, encode it. A keeper's dependency set is part of its
//! attack surface, and this file is small enough to read in full.
//!
//! Only the legacy format is produced. Address lookup tables would let a
//! keeper reference more accounts than fit here, but no instruction it sends
//! comes close to the limit, and the versioned format is worth adding when
//! something needs it rather than in advance.

use std::{error::Error, fmt};

/// An account reference in an instruction, before the message is compiled.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AccountMeta {
    pub pubkey: [u8; 32],
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn readonly(pubkey: [u8; 32]) -> Self {
        Self {
            is_signer: false,
            is_writable: false,
            pubkey,
        }
    }

    pub fn writable(pubkey: [u8; 32]) -> Self {
        Self {
            is_signer: false,
            is_writable: true,
            pubkey,
        }
    }

    pub fn signer(pubkey: [u8; 32]) -> Self {
        Self {
            is_signer: true,
            is_writable: true,
            pubkey,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Instruction {
    pub program_id: [u8; 32],
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub enum TransactionError {
    TooManyAccounts(usize),
    MissingFeePayer,
    UnknownAccount(String),
}

impl fmt::Display for TransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyAccounts(count) => write!(
                formatter,
                "{count} accounts exceeds what a legacy message can index"
            ),
            Self::MissingFeePayer => write!(formatter, "no fee payer was supplied"),
            Self::UnknownAccount(key) => {
                write!(formatter, "instruction references unlisted account {key}")
            }
        }
    }
}

impl Error for TransactionError {}

/// Solana's compact-u16: seven bits per byte, high bit continues.
fn push_compact_u16(output: &mut Vec<u8>, mut value: u16) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            output.push(byte);
            return;
        }
        byte |= 0x80;
        output.push(byte);
    }
}

/// Compile instructions into a legacy message.
///
/// Account ordering is not cosmetic: the runtime reads the header counts and
/// the position of each key to decide what is a signer and what is writable,
/// so the four groups must appear in this exact order — writable signers,
/// readonly signers, writable non-signers, readonly non-signers — with the fee
/// payer first.
pub fn compile_message(
    fee_payer: [u8; 32],
    instructions: &[Instruction],
    recent_blockhash: [u8; 32],
) -> Result<Vec<u8>, TransactionError> {
    if instructions.is_empty() {
        return Err(TransactionError::MissingFeePayer);
    }

    // Merge duplicate references, keeping the strongest privilege each
    // account was requested with anywhere in the transaction.
    let mut keys: Vec<AccountMeta> = vec![AccountMeta::signer(fee_payer)];
    let mut merge = |meta: &AccountMeta| {
        if let Some(existing) = keys.iter_mut().find(|entry| entry.pubkey == meta.pubkey) {
            existing.is_signer |= meta.is_signer;
            existing.is_writable |= meta.is_writable;
        } else {
            keys.push(meta.clone());
        }
    };
    for instruction in instructions {
        for meta in &instruction.accounts {
            merge(meta);
        }
    }
    // A program id is always referenced readonly, and never as a signer.
    for instruction in instructions {
        merge(&AccountMeta::readonly(instruction.program_id));
    }

    keys[1..].sort_by_key(|meta| match (meta.is_signer, meta.is_writable) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    });

    if keys.len() > 255 {
        return Err(TransactionError::TooManyAccounts(keys.len()));
    }

    let signers = keys.iter().filter(|meta| meta.is_signer).count() as u8;
    let readonly_signers = keys
        .iter()
        .filter(|meta| meta.is_signer && !meta.is_writable)
        .count() as u8;
    let readonly_others = keys
        .iter()
        .filter(|meta| !meta.is_signer && !meta.is_writable)
        .count() as u8;

    let index_of = |pubkey: &[u8; 32]| -> Result<u8, TransactionError> {
        keys.iter()
            .position(|meta| &meta.pubkey == pubkey)
            .map(|position| position as u8)
            .ok_or_else(|| TransactionError::UnknownAccount(bs58::encode(pubkey).into_string()))
    };

    let mut message = vec![signers, readonly_signers, readonly_others];
    push_compact_u16(&mut message, keys.len() as u16);
    for meta in &keys {
        message.extend_from_slice(&meta.pubkey);
    }
    message.extend_from_slice(&recent_blockhash);

    push_compact_u16(&mut message, instructions.len() as u16);
    for instruction in instructions {
        message.push(index_of(&instruction.program_id)?);
        push_compact_u16(&mut message, instruction.accounts.len() as u16);
        for meta in &instruction.accounts {
            message.push(index_of(&meta.pubkey)?);
        }
        push_compact_u16(&mut message, instruction.data.len() as u16);
        message.extend_from_slice(&instruction.data);
    }
    Ok(message)
}

/// A signed transaction: the signature vector, then the message.
pub fn serialize_transaction(signatures: &[[u8; 64]], message: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(1 + signatures.len() * 64 + message.len());
    push_compact_u16(&mut output, signatures.len() as u16);
    for signature in signatures {
        output.extend_from_slice(signature);
    }
    output.extend_from_slice(message);
    output
}

/// Base64, standard alphabet with padding. Small enough not to justify a
/// dependency, and used only on the way out to the RPC node.
pub fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let packed = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | bytes[2] as u32;
        output.push(ALPHABET[(packed >> 18) as usize & 0x3f] as char);
        output.push(ALPHABET[(packed >> 12) as usize & 0x3f] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(packed >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[packed as usize & 0x3f] as char
        } else {
            '='
        });
    }
    output
}

/// Compile a v0 message against an already validated, active lookup table.
/// Invoked program IDs and signers always remain in the static key list.
pub fn compile_v0_message(
    fee_payer: [u8; 32],
    instructions: &[Instruction],
    recent_blockhash: [u8; 32],
    table: [u8; 32],
    addresses: &[[u8; 32]],
) -> Result<Vec<u8>, TransactionError> {
    let legacy = compile_message(fee_payer, instructions, recent_blockhash)?;
    let mut offset = 3;
    let mut count = 0usize;
    let mut shift = 0;
    loop {
        let byte = legacy[offset];
        offset += 1;
        count |= ((byte & 127) as usize) << shift;
        if byte & 128 == 0 {
            break;
        }
        shift += 7;
    }
    let keys: Vec<[u8; 32]> = legacy[offset..offset + count * 32]
        .chunks_exact(32)
        .map(|c| c.try_into().unwrap())
        .collect();
    let signers = legacy[0] as usize;
    let ro_signers = legacy[1] as usize;
    let ro_others = legacy[2] as usize;
    let programs: Vec<_> = instructions.iter().map(|i| i.program_id).collect();
    let mut statics = Vec::new();
    let mut writable = Vec::new();
    let mut readonly = Vec::new();
    for (index, k) in keys.iter().enumerate() {
        let writable_flag = if index < signers {
            index < signers - ro_signers
        } else {
            index < count - ro_others
        };
        if index >= signers
            && !programs.contains(k)
            && let Some(i) = addresses.iter().position(|a| a == k)
        {
            let i =
                u8::try_from(i).map_err(|_| TransactionError::TooManyAccounts(addresses.len()))?;
            if writable_flag {
                writable.push((i, *k));
            } else {
                readonly.push((i, *k));
            }
            continue;
        }
        statics.push(*k);
    }
    let all: Vec<_> = statics
        .iter()
        .copied()
        .chain(writable.iter().map(|(_, k)| *k))
        .chain(readonly.iter().map(|(_, k)| *k))
        .collect();
    let mut out = vec![
        0x80,
        signers as u8,
        ro_signers as u8,
        (ro_others - readonly.len()) as u8,
    ];
    push_compact_u16(&mut out, statics.len() as u16);
    for k in statics {
        out.extend(k);
    }
    out.extend(recent_blockhash);
    push_compact_u16(&mut out, instructions.len() as u16);
    for ix in instructions {
        out.push(all.iter().position(|k| *k == ix.program_id).unwrap() as u8);
        push_compact_u16(&mut out, ix.accounts.len() as u16);
        for a in &ix.accounts {
            out.push(all.iter().position(|k| *k == a.pubkey).unwrap() as u8);
        }
        push_compact_u16(&mut out, ix.data.len() as u16);
        out.extend(&ix.data);
    }
    out.push(1);
    out.extend(table);
    push_compact_u16(&mut out, writable.len() as u16);
    out.extend(writable.iter().map(|(i, _)| i));
    push_compact_u16(&mut out, readonly.len() as u16);
    out.extend(readonly.iter().map(|(i, _)| i));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    #[test]
    fn compact_u16_matches_the_wire_format() {
        let mut output = Vec::new();
        push_compact_u16(&mut output, 0);
        push_compact_u16(&mut output, 127);
        push_compact_u16(&mut output, 128);
        push_compact_u16(&mut output, 16_384);
        assert_eq!(output, vec![0, 127, 0x80, 0x01, 0x80, 0x80, 0x01]);
    }

    #[test]
    fn fee_payer_is_first_and_privileges_are_grouped() {
        let message = compile_message(
            key(1),
            &[Instruction {
                accounts: vec![
                    AccountMeta::readonly(key(4)),
                    AccountMeta::writable(key(3)),
                    AccountMeta::signer(key(2)),
                ],
                data: vec![9],
                program_id: key(9),
            }],
            key(0),
        )
        .unwrap();

        // Two writable signers, no readonly signers, two readonly others
        // (the readonly account and the program id).
        assert_eq!(&message[..3], &[2, 0, 2]);
        assert_eq!(&message[4..36], &key(1));
        assert_eq!(&message[36..68], &key(2));
        assert_eq!(&message[68..100], &key(3));
    }

    #[test]
    fn duplicate_references_keep_the_strongest_privilege() {
        let message = compile_message(
            key(1),
            &[Instruction {
                accounts: vec![AccountMeta::readonly(key(5)), AccountMeta::writable(key(5))],
                data: vec![],
                program_id: key(9),
            }],
            key(0),
        )
        .unwrap();
        // One signer, and key(5) merged into a single writable entry, so the
        // account count is fee payer + key(5) + program id.
        assert_eq!(message[3], 3);
        assert_eq!(&message[..3], &[1, 0, 1]);
    }

    #[test]
    fn a_program_id_is_never_promoted_to_signer() {
        let message = compile_message(
            key(1),
            &[Instruction {
                accounts: vec![],
                data: vec![],
                program_id: key(9),
            }],
            key(0),
        )
        .unwrap();
        assert_eq!(&message[..3], &[1, 0, 1]);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
