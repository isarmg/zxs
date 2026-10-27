//! Metadata-only directory composition views; never copy namespace entries.
use crate::State;
use std::cmp::Reverse;

pub struct DirectoryBranchScope<'a> {
    pub repository_id: &'a str,
    pub directory_id: &'a str,
    pub path: &'a str,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryBranchSummary {
    pub directory_branch_id: String,
    pub name: String,
    pub path: String,
    pub current: bool,
    pub materialized: bool,
    pub head_tree_id: String,
    pub file_count: usize,
    pub parent_branch_id: Option<String>,
}

/// Project the specified scope without interpreting or validating caller IDs.
/// Admission and path resolution belong to the actual request boundary.
pub fn directory_branch_summaries(
    state: &State,
    scope: DirectoryBranchScope<'_>,
) -> Vec<DirectoryBranchSummary> {
    let selected = state.directory_selections.get(scope.directory_id);
    let mut branches = state
        .directory_branches
        .values()
        .filter(|branch| {
            branch.repository_id == scope.repository_id && branch.directory_id == scope.directory_id
        })
        .collect::<Vec<_>>();
    branches.sort_by_key(|branch| {
        (
            Reverse(branch.sequence),
            branch.directory_branch_id.as_str(),
        )
    });
    branches
        .into_iter()
        .map(|branch| {
            let crate::DirectoryBranch {
                directory_branch_id,
                repository_id: _,
                directory_id: _,
                path: _,
                name,
                parent_branch_id,
                head_tree_id,
                commit_id: _,
                entries: _,
                file_branches,
                file_contexts: _,
                inherited_files: _,
                directory_refs: _,
                materialized,
                sequence: _,
            } = branch;
            DirectoryBranchSummary {
                directory_branch_id: directory_branch_id.clone(),
                name: name.clone(),
                path: scope.path.into(),
                current: selected == Some(directory_branch_id),
                materialized: *materialized,
                head_tree_id: head_tree_id.clone(),
                file_count: file_branches.len(),
                parent_branch_id: parent_branch_id.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../test/contract/unit/directory_branch_summary.rs"]
mod tests;
