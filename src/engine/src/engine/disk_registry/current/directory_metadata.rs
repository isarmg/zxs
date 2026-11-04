//! A directory label change keeps the exact admitted tree projection.
use super::*;
use crate::DirectoryBranch;

fn same_directory_contents(before: &DirectoryBranch, after: &DirectoryBranch) -> bool {
    // Exhaustive destructuring makes every new DirectoryBranch field an explicit
    // decision. Only the public label is independent of snapshot contents.
    let DirectoryBranch {
        directory_branch_id,
        repository_id,
        directory_id,
        path,
        name: _,
        parent_branch_id,
        head_tree_id,
        commit_id,
        entries,
        file_branches,
        file_contexts,
        inherited_files,
        directory_refs,
        materialized,
        sequence,
    } = before;
    directory_branch_id == &after.directory_branch_id
        && repository_id == &after.repository_id
        && directory_id == &after.directory_id
        && path == &after.path
        && parent_branch_id == &after.parent_branch_id
        && head_tree_id == &after.head_tree_id
        && commit_id == &after.commit_id
        && entries == &after.entries
        && file_branches == &after.file_branches
        && file_contexts == &after.file_contexts
        && inherited_files == &after.inherited_files
        && directory_refs == &after.directory_refs
        && materialized == &after.materialized
        && sequence == &after.sequence
}

impl CurrentNamespaces {
    pub(super) fn directory_label_preserves_entries(
        &self,
        previous: &State,
        next: &State,
        before: &DirectoryBranch,
        after: &DirectoryBranch,
    ) -> Result<bool> {
        if !same_directory_contents(before, after) {
            return Ok(false);
        }
        self.directory_entries(&before.directory_branch_id, &before.head_tree_id)?;
        let original = previous
            .directory_trees
            .get(&before.head_tree_id)
            .ok_or_else(stale)?;
        let retained = next
            .directory_trees
            .get(&after.head_tree_id)
            .ok_or_else(stale)?;
        if !std::ptr::eq(original, retained) && original != retained {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "directory label change rewrites its immutable head tree",
            ));
        }
        Ok(true)
    }
}
