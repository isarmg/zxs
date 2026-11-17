//! Mount names are resolved only from admitted original reservations. Pending
//! reservations occupy their names; local filesystem names never create a route.
use crate::business::namespace::working::WorkingNamespace;
use crate::snapshot_worktree::registry::{NameKey, Publication, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use name_policy::LookupName;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::snapshot_worktree::{NamespaceDirectoryId, NamespaceHostAddress, WorktreeHandle};
use storage_contract::working_tree::RootBinding;
use storage_contract::RepositoryId;

impl ReceiptIndex<'_> {
    /// Pending and completed mounts occupy the ordinary parent's namespace.
    /// Consult the admitted original claims before publishing an ordinary
    /// candidate, not only when a later SMB read discovers an overlap.
    pub(in crate::snapshot_worktree) fn require_mount_namespace(
        &self, next: &crate::State, maximum: usize,
    ) -> Result<()> {
        let mut claims: BTreeMap<RepositoryId, BTreeSet<NameKey>> = BTreeMap::new();
        for record in self.worktrees.values() {
            let plan = &record.plan;
            let repository = plan.host.namespace.repository_id;
            if plan.host.namespace.directory != NamespaceDirectoryId::Repository(repository)
                || !plan.host.directories.is_empty()
                || plan.request.parent_directory_id != plan.host.common_data.directory_id
            {
                return Err(RegistrationError::Source);
            }
            claims.entry(repository).or_default().insert(NameKey(plan.request.name.key()));
        }
        for (repository, names) in claims {
            let key = repository.to_string();
            let previous = self.source.repositories.get(&key).ok_or(RegistrationError::Source)?;
            let owner = next.repositories.get(&key).ok_or(RegistrationError::Source)?;
            if !owner.file_manager_root || owner.main_branch_id != previous.main_branch_id
                || owner.primary_slot_id != previous.primary_slot_id
                || owner.history_tree_id != previous.history_tree_id
            {
                return Err(RegistrationError::Source);
            }
            let namespace = WorkingNamespace::from_main_state(next, repository, maximum)
                .map_err(RegistrationError::Publication)?;
            for entry in namespace.entries() {
                let key = entry.path().key();
                let component = key.split_once('/').map_or(key, |(name, _)| name);
                if names.contains(&NameKey(component.to_owned())) {
                    return Err(RegistrationError::Mount);
                }
            }
            if next.repositories.values().any(|candidate| {
                !candidate.file_manager_root && candidate.primary_slot_id == owner.primary_slot_id
                    && names.contains(&NameKey(name_policy::lookup_key(&candidate.name)))
            }) {
                return Err(RegistrationError::Mount);
            }
        }
        Ok(())
    }

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
