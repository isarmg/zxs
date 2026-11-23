//! Fixed, local maintenance for the sole shared Web/SMB identity in exact v20;
//! no retired SQL SMB credential issuer, migration or protocol fallback.
mod command;
mod native_run;
mod native_sql;
#[cfg(test)]
pub(crate) use native_run::exec_first_browser;
mod fresh_issuer;
mod import_source;
mod observed;
mod retire_ordinary;
mod rust_enroll;
mod smb_policy;
mod terminal;
mod web_secret;
pub(super) use fresh_issuer::{ExplicitSubject, compiled_schema};
pub(crate) use native_run::{rust_account_worker, rust_vault_worker};
mod domain;
#[cfg(test)]
mod recycle;

use super::{bad, request::Id};
pub(super) use domain::require_lent_ex;
pub(crate) use native_run::exec_layout_sql_child;
pub(super) use native_sql::WriteGate as NativeWriteGate;

fn login(s: &[u8]) -> bool {
    let basic = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (1..=64).contains(&s.len())
        && basic(s[0])
        && s.iter()
            .all(|&b| basic(b) || matches!(b, b'.' | b'_' | b'-'))
}
