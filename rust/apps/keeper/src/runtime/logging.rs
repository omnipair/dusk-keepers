//! Terminal presentation only; never used by execution or accounting decisions.
use std::io::{IsTerminal, Write};

pub const GREEN: &str = "32";
pub const RED: &str = "31";
pub const YELLOW: &str = "33";
pub const CYAN: &str = "36";
pub const DIM: &str = "2";

pub fn color_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match std::env::var("KEEPER_LOG_COLOR").as_deref() {
        Ok("always") => true,
        Ok("never") => false,
        _ => std::io::stdout().is_terminal(),
    }
}

pub fn paint(text: impl AsRef<str>, color: &str) -> String {
    let text = text.as_ref();
    if color_enabled() {
        format!("\x1b[{color}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Logging must not unwind a submission/reconciliation if the output pipe closes.
pub fn emit(text: impl AsRef<str>) {
    let _ = writeln!(std::io::stdout().lock(), "{}", text.as_ref());
}

pub fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

pub fn units(value: u128, decimals: u8) -> String {
    if decimals == 0 {
        return value.to_string();
    }
    let mut digits = format!("{value:0>width$}", width = usize::from(decimals) + 1);
    digits.insert(digits.len() - usize::from(decimals), '.');
    digits
}

pub fn signed(value: i128, decimals: u8) -> String {
    format!(
        "{}{amount}",
        if value < 0 { "-" } else { "+" },
        amount = units(value.unsigned_abs(), decimals)
    )
}
