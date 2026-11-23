//! Host-side decisions and the authenticated result for the private v20
//! handoff. No mode can choose media by parsing a UUID or receipt.
mod transport;
pub(super) mod joint_material;
mod manifest;
mod worker;
mod layout;
mod first_browser;
mod rust_credentials;
mod objects;
mod shared_account_reset;
pub(crate) use shared_account_reset::Selection as SharedAccountReset;
pub(in crate::sqlite_install::pair) use rust_credentials::{DurableRustPending,RustEnrollmentApproval,FreshActive,transact_fresh};
pub(crate) use rust_credentials::{account_worker as rust_account_worker,vault_worker as rust_vault_worker};
mod copy_repair_browser;
pub(super) use copy_repair_browser::offer as offer_copy_repair_web;
pub(crate) use copy_repair_browser::worker as exec_copy_repair_browser;
pub(in crate::sqlite_install) use first_browser::offer as offer_init_web;
pub(super) use first_browser::offer_phase as offer_account_web;
pub(crate) use first_browser::worker as exec_first_browser;
pub(super) use layout::{Choice,FirstShares,FreshScope,ResolvedRust};
pub(crate) use layout::{RetirementCheck,ImportCheck,check_deployment};
pub(crate) use layout::worker as exec_layout_sql_child;
mod host;
use super::bad;
