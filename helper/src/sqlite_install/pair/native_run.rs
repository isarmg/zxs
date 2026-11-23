//! Host-side decisions and the authenticated result for the private v20
//! handoff. No mode can choose media by parsing a UUID or receipt.
use crate::local_worker::transport;
mod first_browser;
pub(super) mod joint_material;
mod layout;
mod manifest;
mod objects;
mod rust_credentials;
#[cfg(test)]
mod shared_account_reset;
mod worker;
pub(in crate::sqlite_install) use first_browser::offer as offer_init_web;
pub(crate) use first_browser::worker as exec_first_browser;
#[cfg(test)]
pub(super) use layout::Choice;
pub(crate) use layout::worker as exec_layout_sql_child;
pub(crate) use rust_credentials::{
    account_worker as rust_account_worker, vault_worker as rust_vault_worker,
};
mod host;
use super::bad;
