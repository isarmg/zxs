//! Borrowed composition payloads, independent of branch identity and publication metadata.
use std::collections::BTreeMap;
use storage_contract::{DirectoryBranch, DirectoryReference, DirectoryTree, Entry, FileContext};

/// A view of one exact branch or frozen tree, not proof of admission or ownership.
/// Callers retain the owner separately and validate the same source before use.
pub(crate) struct DirectoryContents<'a> {
    pub(crate) commit_id: &'a str,
    pub(crate) path: &'a str,
    pub(crate) entries: &'a [Entry],
    pub(crate) file_branches: &'a BTreeMap<String, String>,
    pub(crate) file_contexts: &'a BTreeMap<String, FileContext>,
    pub(crate) inherited_files: &'a BTreeMap<String, String>,
    pub(crate) directory_refs: &'a BTreeMap<String, DirectoryReference>,
}

impl<'a> From<&'a DirectoryBranch> for DirectoryContents<'a> {
    fn from(branch: &'a DirectoryBranch) -> Self {
        let DirectoryBranch {
            directory_branch_id: _,
            repository_id: _,
            directory_id: _,
            name: _,
            parent_branch_id: _,
            head_tree_id: _,
            materialized: _,
            sequence: _,
            commit_id,
            path,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
        } = branch;
        Self {
            commit_id,
            path,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
        }
    }
}

impl<'a> From<&'a DirectoryTree> for DirectoryContents<'a> {
    fn from(tree: &'a DirectoryTree) -> Self {
        let DirectoryTree {
            tree_id: _,
            directory_branch_id: _,
            parent_tree_id: _,
            sequence: _,
            commit_id,
            path,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
        } = tree;
        Self {
            commit_id,
            path,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/directory_contents_tests.rs"]
mod tests;
