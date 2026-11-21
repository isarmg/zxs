//! Original on-demand product deployment custody; no runtime server.
mod provision;
pub(super) use provision::{DeployedSite,deployed_worker_payload,verify_deployed_worker,FreshHome,FreshParent,Allocated,CurrentVault,PlacedVault,
    request_live_approval,give_live_approval,CredentialSite,RustSite,VaultStamp,checked_vault_stamp,verify_fresh_worker,sealed};
pub(in crate::sqlite_install) use provision::{require_lent_ex,require_lent_sh};
use super::bad;
