//! Automatic intent and chosen mount are independently bound by the original
//! reservation. Pending names stay occupied but do not authorize publication.
use crate::directory_worktree::names;
use crate::persist::snapshot_worktree::{AllocatedWorktreeReservation, HostEvidence};
use crate::snapshot_worktree::registry::{
    Allocations, CreationIntent, Publication, Receipt, ReceiptIndex,
};
use crate::snapshot_worktree::{RegistrationError, Result};
use name_policy::LookupName;
use storage_contract::Principal;
use storage_contract::snapshot_worktree::WorktreeHandle;
use storage_contract::snapshot_worktree::creation::AllocateWorktree;
use storage_contract::working_tree::{DirectoryId, MaterialCommitment};

impl ReceiptIndex<'_> {
    pub(in crate::snapshot_worktree) fn retry_allocation(
        &self,
        request: &AllocateWorktree,
        principal: &Principal,
    ) -> Result<Option<WorktreeHandle>> {
        if !self.source.operations.contains_key(&request.operation_id) {
            return Ok(None);
        }
        let tree = self
            .worktrees
            .get(&request.operation_id)
            .ok_or(RegistrationError::Operation(request.operation_id))?;
        let CreationIntent::Automatic(original) = &tree.intent else {
            return Err(RegistrationError::Digest(request.operation_id));
        };
        if original != request || &tree.receipt.principal != principal {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        let Publication::Complete(_) = &tree.publication else {
            return Err(RegistrationError::Pending(request.operation_id));
        };
        Ok(Some(WorktreeHandle {
            publication: tree.plan.allocated.completion,
            worktree_id: tree.plan.allocated.worktree,
        }))
    }

    pub(in crate::snapshot_worktree) fn reserved_mount_names(
        &self,
        host: &HostEvidence,
        parent: DirectoryId,
    ) -> Vec<LookupName> {
        self.worktrees
            .values()
            .filter(|record| {
                record.plan.host.registration == host.registration
                    && record.plan.host.root_entity == host.root_entity
                    && record.plan.request.parent_directory_id == parent
            })
            .map(|record| LookupName::from(&record.plan.request.name))
            .collect()
    }
}

impl<'a> ReceiptIndex<'a> {
    pub(super) fn reserve_allocated_worktree(
        &mut self,
        receipt: Receipt<'a>,
        plan: AllocatedWorktreeReservation,
        commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let request = &plan.request;
        let resolved = &plan.resolved.request;
        if request.operation_id != resolved.operation_id
            || request.source != resolved.source
            || request.parent_directory_id != resolved.parent_directory_id
            || request.expected_root_generation != resolved.expected_root_generation
            || !names::is_allocated_name(self.node(request.source)?.branch_name(), &resolved.name)
        {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        self.reserve_worktree(
            receipt,
            plan.resolved,
            CreationIntent::Automatic(plan.request),
            commitment,
            allocations,
        )
    }
}
