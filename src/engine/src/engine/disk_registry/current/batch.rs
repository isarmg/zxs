//! A batch may mutate selected current compositions, never import a frozen tree.
use super::*;
use crate::DirectoryBranch;

impl CurrentNamespaces {
    /// Keep unsupported inactive/history transfers behind the fence. The exact
    /// selected subtree, including versions, sizes and path spelling, must equal
    /// its owning current namespace. Global entry/file membership is no proof.
    pub(crate) fn require_batch_scope(
        &self,
        state: &State,
        branch: &DirectoryBranch,
    ) -> Result<()> {
        let repository = state
            .repositories
            .get(&branch.repository_id)
            .ok_or_else(stale)?;
        let owner = self.writable_scope(state, &repository.main_branch_id)?;
        let scope = self
            .directories
            .get(&branch.directory_branch_id)
            .filter(|scope| scope.head == branch.head_tree_id)
            .ok_or_else(stale)?;
        if !scope.writable {
            return Err(Error::new(
                ErrorCode::BranchNotActive,
                "batch scope has no retained writable registration",
            ));
        }
        if state.directory_selections.get(&branch.directory_id) != Some(&branch.directory_branch_id)
            || state.directory_tree_selections.get(&branch.directory_id)
                != Some(&branch.head_tree_id)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "batch edits of inactive or historical directory compositions are not admitted",
            ));
        }
        let key = name_policy::path_key(&branch.path);
        let entries = if branch.directory_id == repository.repository_id && branch.path.is_empty() {
            owner.entries.as_slice()
        } else {
            let directory = owner
                .entries
                .binary_search_by(|entry| entry.lookup_key().cmp(&key))
                .ok()
                .map(|index| &owner.entries[index]);
            if !directory.is_some_and(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && entry.entry_id == branch.directory_id
                    && entry.path() == branch.path
            }) {
                return Err(Error::new(
                    ErrorCode::DirectoryBranchConflict,
                    "batch directory identity/path is absent from the current owner",
                ));
            }
            let prefix = format!("{key}/");
            let start = owner
                .entries
                .partition_point(|entry| entry.lookup_key() < prefix.as_str());
            let tail = &owner.entries[start..];
            let end = tail.partition_point(|entry| entry.lookup_key().starts_with(&prefix));
            &tail[..end]
        };
        if scope.entries.as_slice() != entries {
            return Err(Error::new(
                ErrorCode::WorkspaceChanged,
                "selected batch composition differs from its exact current owner occurrences",
            ));
        }
        Ok(())
    }
}
