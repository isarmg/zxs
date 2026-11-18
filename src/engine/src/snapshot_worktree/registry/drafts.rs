//! Directory selection borrows a complete published head, never a reserved node.
use crate::snapshot_worktree::registry::{PublishedNode, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use storage_contract::snapshot_worktree::SnapshotHandle;

impl ReceiptIndex<'_> {
    /// Pending and completed nested registrations both occupy their original
    /// namespace entry. Never discover a host from a local management filename.
    pub(crate) fn contains_nested_draft_host(
        &self, repository: storage_contract::RepositoryId, branch: storage_contract::BranchId,
        parents: &[(storage_contract::EntryId, name_policy::PortablePath)],
    ) -> bool {
        use storage_contract::snapshot_worktree::NamespaceDirectoryId;
        !parents.is_empty() && self.nodes.values().any(|record| {
            let ns = &record.plan.host.namespace;
            ns.repository_id == repository && ns.branch_id == branch
                && matches!(ns.directory, NamespaceDirectoryId::Entry(id)
                    if parents.iter().any(|(parent, _)| *parent == id))
        })
    }

    /// Check a mapping witness only against *its original* immediately prior
    /// native publication cut. The authenticated mapping issuer owns the
    /// request, actor, exact instance/occurrence, chronology and decision.
    /// No FILE bytes or physical read authority come from this predicate.
    pub(crate) fn require_outside_selection_at_issue(
        &self, previous: &crate::State, config: &crate::ValidatedConfig,
        witness: &crate::persist::snapshot_worktree::outside_selection::OutsideSelectionWitness,
    ) -> crate::Result<()> {
        self.require_source(previous).map_err(crate::business::file_draft::worktree::failure)?;
        let source = crate::engine::fresh_layout::namespace::outside::original(
            previous, config, witness,
        )?;
        let selected = witness.fields();
        if self.contains_nested_draft_host(
            selected.target.repository, selected.namespace_branch, &source.parents,
        ) {
            return Err(crate::engine::fresh_layout::FreshLayoutError::ContainerScope.into());
        }
        Ok(())
    }

    pub(in crate::snapshot_worktree) fn draft_head(
        &self,
        handle: SnapshotHandle,
    ) -> Result<PublishedNode<'_>> {
        let published = self.node(handle)?;
        let branch = (handle.repository_id, handle.branch_id);
        if let Some(operation) = self.pending_branches.get(&branch) {
            return Err(RegistrationError::Pending(*operation));
        }
        if self.branch_heads.get(&branch) != Some(&handle) {
            return Err(RegistrationError::Source);
        }
        Ok(published)
    }
}
