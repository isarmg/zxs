//! Mount names are resolved only from admitted original reservations. Pending
//! reservations occupy their names; local filesystem names never create a route.
use crate::business::namespace::working::WorkingNamespace;
use crate::persist::snapshot_worktree::{Record, WorktreeReservation};
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
        let plans = self.worktrees.values().map(|record| &record.plan);
        self.require_unoccupied(next, maximum, plans)
    }

    /// The ordinary publication fence sees only *previously* registered mounts.
    /// A fresh claim must also respect names already owned by the ordinary root
    /// before its first permanent reservation, not only on later completion.
    pub(in crate::snapshot_worktree) fn require_new_mount_namespace(
        &self, next: &crate::State, maximum: usize, record: &Record,
    ) -> Result<()> {
        let plan = match record {
            Record::ReserveWorktree(plan) => plan,
            Record::ReserveAllocatedWorktree(plan) => &plan.resolved,
            Record::ReserveNode(_) | Record::CompleteNode(_) | Record::CompleteWorktree(_)
            | Record::ReserveEdit(_) | Record::CompleteEdit(_) | Record::ReserveCommit(_)
            | Record::ReserveHistoricalCommit(_) | Record::PrepareHistoricalCommit(_)
            | Record::PublishCommitNode(_) | Record::CompleteCommit(_) | Record::BirthFile(_)
            | Record::SaveFile(_) | Record::CompleteFile(_) | Record::ReserveAddedCommit(_)
            | Record::ReserveContentBatch(_) | Record::PublishContentBatch(_)
            | Record::PublishAddedFile(_) | Record::PublishAddedNode(_)
            | Record::CompleteAddedCommit(_) => return Ok(()),
        };
        self.require_unoccupied(next, maximum, std::iter::once(plan))
    }

    fn require_unoccupied<'p>(
        &self, next: &crate::State, maximum: usize,
        plans: impl Iterator<Item = &'p WorktreeReservation>,
    ) -> Result<()> {
        let mut claims: BTreeMap<RepositoryId, BTreeSet<NameKey>> = BTreeMap::new();
        for plan in plans {
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
