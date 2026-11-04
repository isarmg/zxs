//! Admit a genuinely new empty root without granting historical imports.
use super::*;
use crate::{Branch, BranchKind, HistoryState};

impl CurrentNamespaces {
    pub(super) fn new_branch_writable(
        &self,
        previous: &State,
        next: &State,
        branch: &Branch,
        commit: &Commit,
    ) -> Result<bool> {
        let history = next
            .histories
            .get(&branch.history_tree_id)
            .ok_or_else(stale)?;
        let repository = next
            .repositories
            .get(&history.repository_id)
            .ok_or_else(stale)?;
        if let Some(owner) = previous.repositories.get(&repository.repository_id) {
            // Existing helpers retain the same actual repository/main binding.
            // A new repository cannot borrow another root's writable scope.
            if owner.history_tree_id != repository.history_tree_id
                || owner.main_branch_id != repository.main_branch_id
            {
                return Err(stale());
            }
            return self.branch_writable(previous, &repository.main_branch_id);
        }
        if previous.histories.contains_key(&history.history_tree_id)
            || previous.branches.contains_key(&branch.branch_id)
            || previous.commits.contains_key(&commit.commit_id)
            || repository.main_branch_id != branch.branch_id
            || repository.history_tree_id != history.history_tree_id
            || history.root_commit_id != commit.commit_id
            || history.main_branch_id.as_deref() != Some(branch.branch_id.as_str())
            || history.state != HistoryState::Active
            || history.provenance.is_some()
            || branch.state != BranchState::Active
            || branch.branch_kind != BranchKind::Full
            || branch.file_id.is_some()
            || branch.file_node_id.is_some()
            || branch.head_commit_id != commit.commit_id
            || branch.fork_commit_id != commit.commit_id
            || !branch.pending_file_versions.is_empty()
            || commit.history_tree_id != history.history_tree_id
            || commit.kind != "root"
            || !commit.parent_commit_ids.is_empty()
            || !commit.entries.is_empty()
            || !commit.pending_file_versions.is_empty()
            || commit.namespace_deletions.is_some()
            || commit.provenance.is_some()
            || commit.sequence != next.sequence
        {
            return Err(Error::new(
                ErrorCode::WorkspaceChanged,
                "new namespace is not a fresh empty repository root",
            ));
        }
        if !self
            .registry_state
            .registered_definitions()
            .any(|(disk, mode, _)| {
                disk.identity.slot_id == repository.primary_slot_id
                    && disk.level == 1
                    && mode == super::super::RegistrationMode::Enabled
            })
        {
            return Err(Error::new(
                ErrorCode::PlacementInvalid,
                "new repository has no enabled registered primary owner",
            ));
        }
        Ok(true)
    }
}
