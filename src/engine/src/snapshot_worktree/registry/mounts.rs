//! Mount names are resolved only from admitted original reservations. Pending
//! reservations occupy their names; local filesystem names never create a route.
use crate::snapshot_worktree::registry::{Publication, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use name_policy::LookupName;
use storage_contract::snapshot_worktree::{NamespaceHostAddress, WorktreeHandle};
use storage_contract::working_tree::RootBinding;

impl ReceiptIndex<'_> {
    pub(in crate::snapshot_worktree) fn has_mount(
        &self,
        repository: storage_contract::RepositoryId,
        name: Option<&LookupName>,
    ) -> bool {
        self.worktrees.values().any(|record| {
            record.plan.host.namespace.repository_id == repository
                && name.is_none_or(|name| name.key() == record.plan.request.name.key())
        })
    }

    pub(in crate::snapshot_worktree) fn mount_handles(
        &self,
        namespace: NamespaceHostAddress,
        root: &RootBinding,
        name: Option<&LookupName>,
        maximum: usize,
    ) -> Result<Vec<WorktreeHandle>> {
        let mut handles = Vec::new();
        for record in self.worktrees.values() {
            let plan = &record.plan;
            if plan.host.namespace.repository_id != namespace.repository_id
                || name.is_some_and(|name| name.key() != plan.request.name.key())
            {
                continue;
            }
            if plan.host.namespace != namespace
                || plan.host.registration != root.registration
                || plan.host.root_entity != root.root_entity
                || plan.host.common_data != root.roots.common_data
                || !plan.host.directories.is_empty()
                || plan.request.parent_directory_id != root.roots.common_data.directory_id
                || plan.request.expected_root_generation != root.generation
            {
                return Err(RegistrationError::Source);
            }
            if matches!(record.publication, Publication::Reserved) {
                return Err(RegistrationError::Pending(
                    record.receipt.original.operation_id,
                ));
            }
            if handles.len() == maximum {
                return Err(RegistrationError::ReadView(
                    crate::directory_worktree::io::ViewError::Budget,
                ));
            }
            handles.push(WorktreeHandle {
                publication: plan.allocated.completion,
                worktree_id: plan.allocated.worktree,
            });
        }
        Ok(handles)
    }
}
