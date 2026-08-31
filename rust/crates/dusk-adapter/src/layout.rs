//! Account field offsets, generated from the pinned IDL.
//!
//! A keeper reads a handful of fields out of accounts with dozens. Decoding
//! the whole account would mean a second Rust model of every protocol struct,
//! free to drift from the program with each release; hand-counting offsets is
//! worse still, because `Market.quote_side.asset_mint` sits behind a nested
//! struct nobody can size by eye and a wrong offset is a silent wrong answer
//! rather than an error.
//!
//! So the offsets come from `scripts/generate-account-layout.mjs`, which
//! computes them from the IDL the lock pins, and this module reads them.

use std::{collections::BTreeMap, error::Error, fmt};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLayoutManifest {
    pub schema_version: u8,
    pub protocol_revision: String,
    pub program_id: String,
    pub idl_sha256: String,
    pub accounts: Vec<AccountLayout>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLayout {
    pub name: String,
    /// The account's exact size, when it has one.
    #[serde(default)]
    pub size_with_discriminator: Option<usize>,
    /// The least an account must be for the requested fields to exist, for
    /// accounts carrying a data-bearing enum that gives them no single size.
    #[serde(default)]
    pub minimum_size: Option<usize>,
    pub fields: Vec<FieldLocation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldLocation {
    pub path: String,
    pub offset: usize,
    pub size: usize,
}

#[derive(Debug)]
pub enum LayoutError {
    Malformed(String),
    RevisionMismatch { expected: String, found: String },
    UnknownAccount(String),
    UnknownField(String),
    ShortAccount { expected: usize, found: usize },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(detail) => write!(formatter, "account layout is malformed: {detail}"),
            Self::RevisionMismatch { expected, found } => write!(
                formatter,
                "account layout is pinned to {found}, not {expected}"
            ),
            Self::UnknownAccount(name) => write!(formatter, "account layout has no {name}"),
            Self::UnknownField(path) => write!(formatter, "account layout has no field {path}"),
            Self::ShortAccount { expected, found } => write!(
                formatter,
                "account is {found} bytes, not the {expected} the layout describes"
            ),
        }
    }
}

impl Error for LayoutError {}

/// A single account's fields, keyed by path.
/// How an account's length is checked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeGuard {
    /// The account is exactly this long; anything else is a different account.
    Exact(usize),
    /// The account is at least this long. Weaker, and used only where the
    /// layout genuinely has no fixed size.
    AtLeast(usize),
}

impl SizeGuard {
    fn admits(self, length: usize) -> bool {
        match self {
            Self::Exact(size) => length == size,
            Self::AtLeast(size) => length >= size,
        }
    }

    fn expected(self) -> usize {
        match self {
            Self::AtLeast(size) | Self::Exact(size) => size,
        }
    }
}

pub struct AccountReader<'a> {
    fields: BTreeMap<&'a str, &'a FieldLocation>,
    guard: SizeGuard,
}

impl<'a> AccountReader<'a> {
    /// Bytes for one field.
    ///
    /// The account's total size is checked rather than just the field's end:
    /// a shorter account is a different account, and reading a valid-looking
    /// prefix out of the wrong type is exactly the failure offsets invite.
    pub fn bytes(&self, path: &str, data: &'a [u8]) -> Result<&'a [u8], LayoutError> {
        if !self.guard.admits(data.len()) {
            return Err(LayoutError::ShortAccount {
                expected: self.guard.expected(),
                found: data.len(),
            });
        }
        let field = self
            .fields
            .get(path)
            .ok_or_else(|| LayoutError::UnknownField(path.to_owned()))?;
        Ok(&data[field.offset..field.offset + field.size])
    }

    pub fn pubkey(&self, path: &str, data: &[u8]) -> Result<[u8; 32], LayoutError> {
        let bytes = self.bytes(path, data)?;
        <[u8; 32]>::try_from(bytes)
            .map_err(|_| LayoutError::Malformed(format!("{path} is not 32 bytes")))
    }

    pub fn u64(&self, path: &str, data: &[u8]) -> Result<u64, LayoutError> {
        let bytes = self.bytes(path, data)?;
        <[u8; 8]>::try_from(bytes)
            .map(u64::from_le_bytes)
            .map_err(|_| LayoutError::Malformed(format!("{path} is not 8 bytes")))
    }

    pub fn i128(&self, path: &str, data: &[u8]) -> Result<i128, LayoutError> {
        let bytes = self.bytes(path, data)?;
        <[u8; 16]>::try_from(bytes)
            .map(i128::from_le_bytes)
            .map_err(|_| LayoutError::Malformed(format!("{path} is not 16 bytes")))
    }

    pub fn u8(&self, path: &str, data: &[u8]) -> Result<u8, LayoutError> {
        self.bytes(path, data)?
            .first()
            .copied()
            .ok_or_else(|| LayoutError::Malformed(format!("{path} is empty")))
    }

    /// The length a synthetic account must have to satisfy the guard.
    pub fn size(&self) -> usize {
        self.guard.expected()
    }

    pub fn guard(&self) -> SizeGuard {
        self.guard
    }
}

impl AccountLayoutManifest {
    pub fn from_json(raw: &str) -> Result<Self, LayoutError> {
        serde_json::from_str(raw).map_err(|error| LayoutError::Malformed(error.to_string()))
    }

    /// Reject a layout generated for a different deployment. Offsets from
    /// another revision are not approximately right; they are arbitrary.
    pub fn assert_matches(&self, revision: &str) -> Result<(), LayoutError> {
        if self.protocol_revision != revision {
            return Err(LayoutError::RevisionMismatch {
                expected: revision.to_owned(),
                found: self.protocol_revision.clone(),
            });
        }
        Ok(())
    }

    pub fn reader(&self, account: &str) -> Result<AccountReader<'_>, LayoutError> {
        let layout = self
            .accounts
            .iter()
            .find(|entry| entry.name == account)
            .ok_or_else(|| LayoutError::UnknownAccount(account.to_owned()))?;
        let guard = match (layout.size_with_discriminator, layout.minimum_size) {
            (Some(size), _) => SizeGuard::Exact(size),
            (None, Some(size)) => SizeGuard::AtLeast(size),
            (None, None) => {
                return Err(LayoutError::Malformed(format!(
                    "{account} states neither an exact nor a minimum size"
                )));
            }
        };
        Ok(AccountReader {
            fields: layout
                .fields
                .iter()
                .map(|field| (field.path.as_str(), field))
                .collect(),
            guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: &str = include_str!("../../../../protocol/keeper-account-layout.v1.json");

    fn manifest() -> AccountLayoutManifest {
        AccountLayoutManifest::from_json(LAYOUT).expect("layout must parse")
    }

    #[test]
    fn the_shipped_layout_describes_the_accounts_keepers_read() {
        let manifest = manifest();
        for account in ["BorrowPosition", "Market", "LeveragePosition"] {
            assert!(manifest.reader(account).is_ok(), "{account} is missing");
        }
    }

    /// The two sides must not resolve to the same offset. Nothing else in the
    /// keeper would notice if they did — it would simply act on the wrong
    /// asset, every time, silently.
    #[test]
    fn market_sides_are_distinct() {
        let manifest = manifest();
        let market = manifest.reader("Market").unwrap();
        let data = vec![0_u8; market.size()];
        let base = market.bytes("base_side.asset_mint", &data).unwrap().as_ptr();
        let quote = market.bytes("quote_side.asset_mint", &data).unwrap().as_ptr();
        assert_ne!(base, quote);
    }

    #[test]
    fn an_account_of_the_wrong_size_is_refused() {
        let manifest = manifest();
        let position = manifest.reader("BorrowPosition").unwrap();
        assert!(matches!(
            position.pubkey("owner", &[0_u8; 32]),
            Err(LayoutError::ShortAccount { .. })
        ));
    }

    /// A variable-layout account cannot claim an exact size, and must not
    /// silently borrow the strong guard from the accounts that can.
    #[test]
    fn a_variable_account_is_guarded_by_a_minimum() {
        let manifest = manifest();
        assert!(matches!(
            manifest.reader("BorrowPosition").unwrap().guard(),
            SizeGuard::Exact(_)
        ));
        let proposal = manifest.reader("ParameterProposal").unwrap();
        assert!(matches!(proposal.guard(), SizeGuard::AtLeast(_)));
        // Longer than the minimum is fine; shorter is not.
        let long = vec![0_u8; proposal.size() + 500];
        assert!(proposal.pubkey("market", &long).is_ok());
        let short = vec![0_u8; proposal.size() - 1];
        assert!(proposal.pubkey("market", &short).is_err());
    }

    #[test]
    fn a_layout_from_another_revision_is_refused() {
        assert!(manifest().assert_matches("some-other-revision").is_err());
    }
}
