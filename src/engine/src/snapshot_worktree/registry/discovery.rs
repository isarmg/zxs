//! The unique original host registration chooses the repository; complete
//! publication transitions, not operation order, choose a designated head.
use super::{Publication, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use storage_contract::snapshot_worktree::discovery::{
    SnapshotBranchSelection, SnapshotHeadDiscovery, SnapshotHeadRequest,
};
use storage_contract::working_tree::RootBinding;

impl ReceiptIndex<'_> {
    pub(in crate::snapshot_worktree) fn discover_root(
        &self, request: SnapshotHeadRequest, binding: &RootBinding,
    ) -> Result<SnapshotHeadDiscovery> {
        // Full index admission already rejects a second reservation occupying
        // this host. An incomplete original remains occupied across restart.
        let Some(original) = self.nodes.values().find(|record| {
            record.plan.request.host.root_id == request.host.root_id
                && record.plan.request.host.directory_id == request.host.directory_id
        }) else {
            return Ok(SnapshotHeadDiscovery::NotInitialized);
        };
        let plan = &original.plan;
        if plan.request.host != request.host || plan.host.namespace != request.namespace
            || plan.host.registration != binding.registration
            || plan.host.root_entity != binding.root_entity
            || plan.host.common_data != binding.roots.common_data
            || !plan.host.directories.is_empty()
        {
            return Err(RegistrationError::Source);
        }
        let branch = match request.branch {
            SnapshotBranchSelection::Initial => plan.allocated.branch,
            SnapshotBranchSelection::Exact { repository, branch } => {
                if repository != plan.allocated.repository {
                    return Err(RegistrationError::Source);
                }
                branch
            }
        };
        if matches!(original.publication, Publication::Reserved) {
            if branch != plan.allocated.branch {
                return Err(RegistrationError::Source);
            }
            return Ok(SnapshotHeadDiscovery::Pending {
                operation: original.receipt.original.operation_id,
            });
        }
        let key = (plan.allocated.repository, branch);
        if let Some(operation) = self.pending_branches.get(&key) {
            return Ok(SnapshotHeadDiscovery::Pending { operation: *operation });
        }
        let handle = self.branch_heads.get(&key).copied().ok_or(RegistrationError::Source)?;
        let published = self.node(handle)?;
        Ok(SnapshotHeadDiscovery::Head {
            snapshot: published.handle(), branch_name: published.branch_name().clone(),
        })
    }
}
