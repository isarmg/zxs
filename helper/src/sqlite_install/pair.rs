//! Fixed, local maintenance for the sole shared Web/SMB identity in exact v20;
//! no retired SQL SMB credential issuer, migration or protocol fallback.
mod command;
mod native_sql;
mod native_run;
pub(super) use native_run::offer_init_web;
pub(crate) use native_run::exec_first_browser;
mod web_secret;
mod smb_policy;
mod terminal;
pub(super) use crate::foreground::PublicConfirmation;
mod observed;
mod retire_ordinary;
mod import_source;
pub(crate) use native_run::{RetirementCheck,ImportCheck,check_deployment};
mod rust_enroll;
mod fresh_issuer;
#[cfg(feature="embedded-zxs")]
pub(crate) use fresh_issuer::EmbeddedInstall;
pub(super) use fresh_issuer::{ExplicitSubject,compiled_schema,require_private_install};
pub(crate) use native_run::{rust_account_worker,rust_vault_worker,SharedAccountReset};
mod recycle;
mod domain;

pub(crate) use command::Setup;
pub(crate) use native_run::exec_layout_sql_child;
pub(super) use native_sql::WriteGate as NativeWriteGate;
pub(super) use domain::{InitialLayout,require_lent_ex};
use super::{bad,request::Id};

fn login(s: &[u8]) -> bool {
    let basic = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (1..=64).contains(&s.len()) && basic(s[0])
        && s.iter().all(|&b| basic(b) || matches!(b, b'.' | b'_' | b'-'))
}
