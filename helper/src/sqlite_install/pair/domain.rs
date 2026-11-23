//! Original on-demand product deployment custody; no runtime server.
mod provision;
use super::bad;
pub(in crate::sqlite_install) use provision::require_lent_ex;
pub(super) use provision::{
    VaultStamp, checked_vault_stamp, deployed_worker_payload, request_live_approval, sealed,
    verify_fresh_worker,
};
