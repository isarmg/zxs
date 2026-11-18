//! Mount names are resolved only from admitted original reservations. Pending
//! reservations occupy their names; local filesystem names never create a route.
use crate::business::namespace::working::WorkingNamespace;
use crate::persist::snapshot_worktree::{HostEvidence, Record};
use crate::persist::snapshot_worktree::mount_change::Resolved;
use crate::directory_worktree::lifecycle::Mount;
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
        let plans = self.worktrees.values().filter_map(|record| {
            self.mount_claim(record).map(|mount| (&record.plan.host, mount))
        });
        self.require_unoccupied(next, maximum, plans)
    }

    /// The ordinary publication fence sees only *previously* registered mounts.
    /// A fresh claim must also respect names already owned by the ordinary root
    /// before its first permanent reservation, not only on later completion.
    pub(in crate::snapshot_worktree) fn require_new_mount_namespace(
        &self, next: &crate::State, maximum: usize, record: &Record,
    ) -> Result<()> {
        let (host, mount) = match record {
            Record::ReserveCleanMount(plan) => {
                let mount = match &plan.resolved {
                    Resolved::Hide {} => &plan.prior_mount,
                    Resolved::Restore { mount } => mount,
                };
                (&plan.host, mount.clone())
            }
            Record::ReserveWorktree(plan) => (&plan.host, Mount {
                parent: plan.request.parent_directory_id, name: LookupName::from(&plan.request.name),
            }),
            Record::ReserveAllocatedWorktree(value) => {
                let plan = &value.resolved;
                (&plan.host, Mount {
                    parent: plan.request.parent_directory_id, name: LookupName::from(&plan.request.name),
                })
            }
            Record::ReserveNode(_) | Record::CompleteNode(_) | Record::CompleteWorktree(_)
            | Record::ReserveEdit(_) | Record::CompleteEdit(_) | Record::ReserveCommit(_)
            | Record::ReserveHistoricalCommit(_) | Record::PrepareHistoricalCommit(_)
            | Record::PublishCommitNode(_) | Record::CompleteCommit(_) | Record::BirthFile(_)
            | Record::SaveFile(_) | Record::CompleteFile(_) | Record::ReserveAddedCommit(_)
            | Record::ReserveContentBatch(_) | Record::PublishContentBatch(_) | Record::PublishCleanMount(_)
            | Record::PublishAddedFile(_) | Record::PublishAddedNode(_)
            | Record::CompleteAddedCommit(_) => return Ok(()),
        };
        self.require_unoccupied(next, maximum, std::iter::once((host, mount)))
    }

    fn require_unoccupied<'p>(
        &self, next: &crate::State, maximum: usize,
        plans: impl Iterator<Item = (&'p HostEvidence, Mount)>,
    ) -> Result<()> {
        let mut claims: BTreeMap<RepositoryId, BTreeSet<NameKey>> = BTreeMap::new();
        for (host, mount) in plans {
            let repository = host.namespace.repository_id;
            if host.namespace.directory != NamespaceDirectoryId::Repository(repository)
                || !host.directories.is_empty()
                || mount.parent != host.common_data.directory_id
            {
                return Err(RegistrationError::Source);
            }
            if !claims.entry(repository).or_default().insert(NameKey(mount.name.key())) {
                return Err(RegistrationError::Mount);
            }
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
            ordinary_names(next, repository, maximum, |name| {
                if names.contains(&NameKey(name.key())) { return Err(RegistrationError::Mount); }
                Ok(())
            })?;
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
                && self.mount_claim(record).is_some_and(|mount| {
                    name.is_none_or(|name| name.key() == mount.name.key())
                })
        })
    }

    /// Same candidate-time namespace fence as ordinary writes, restricted to
    /// the selected original mount (including stable named-share / Unborn collisions).
    pub(in crate::snapshot_worktree) fn require_selection_mount(
        &self, host: &HostEvidence, mount: &Mount, root: &RootBinding,
        target: WorktreeHandle, maximum: usize,
    ) -> Result<()> {
        if self.mount_handles(host.namespace, root, Some(&mount.name), 1)?.as_slice() != [target] {
            return Err(RegistrationError::Mount);
        }
        self.require_unoccupied(self.source, maximum, std::iter::once((host, mount.clone())))
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
            {
                continue;
            }
            let Some(mount) = self.mount_claim(record) else { continue; };
            if name.is_some_and(|name| name.key() != mount.name.key())
                || self.clean_change_pending(plan.allocated.worktree)
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

/// The same parent namespace and device-bound logical share occupancy used by
/// the ordinary writer fence, recovery of a reserved name, and cold resolution.
pub(super) fn ordinary_names(
    next: &crate::State, repository: RepositoryId, maximum: usize,
    mut visit: impl FnMut(LookupName) -> Result<()>,
) -> Result<()> {
    let owner = next.repositories.get(&repository.to_string()).ok_or(RegistrationError::Source)?;
    if !owner.file_manager_root { return Err(RegistrationError::Source); }
    let namespace = WorkingNamespace::from_main_state(next, repository, maximum)
        .map_err(RegistrationError::Publication)?;
    for entry in namespace.entries() {
        let path = entry.path().as_str();
        let first = path.split_once('/').map_or(path, |(name,_)| name);
        visit(LookupName::parse(first).map_err(|_| RegistrationError::Mount)?)?;
    }
    for r in next.repositories.values().filter(|r| !r.file_manager_root && r.primary_slot_id == owner.primary_slot_id) {
        visit(LookupName::parse(&r.name).map_err(|_| RegistrationError::Mount)?)?;
    }
    Ok(())
}

impl ReceiptIndex<'_> {
    pub(in crate::snapshot_worktree) fn require_original_restore_name(
        &self, plan: &crate::persist::snapshot_worktree::mount_change::Reservation, maximum: usize,
    ) -> Result<()> {
        use storage_contract::snapshot_worktree::mount_change::{MountAction,RestoreNaming};
        let (MountAction::Restore { naming: RestoreNaming::OriginalOrAutomatic {}, .. }, Resolved::Restore { mount }) =
            (&plan.request.action, &plan.resolved) else { return Ok(()); };
        let mut occupied = self.reserved_mount_names(&plan.host,plan.prior_mount.parent);
        ordinary_names(self.source, plan.host.namespace.repository_id, maximum, |name| { occupied.push(name); Ok(()) })?;
        let expected = if occupied.iter().any(|n| n.key() == plan.prior_mount.name.key()) {
            // Original allocation had to pass the actual device limit. This
            // historical comparison must not re-budget an old decision under
            // a different mount's current component limit.
            LookupName::from(crate::directory_worktree::names::allocate(self.node(plan.source)?.branch_name(),
                occupied,usize::MAX).map_err(RegistrationError::WorktreeName)?.name())
        } else { plan.prior_mount.name.clone() };
        if mount.name != expected { return Err(RegistrationError::Mount); }
        Ok(())
    }
}
