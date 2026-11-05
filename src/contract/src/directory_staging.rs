//! Explicit directory commits are distinct from durable working-tree checkpoints.
use crate::{DirectoryTree, State};
use data_serialization::{Deserialize, Serialize};

/// A user checkpoint is bound to one exact immutable directory tree. Copying or
/// projecting that tree never silently turns the new working tree into a commit.
pub fn directory_tree_is_committed(state: &State, tree: &DirectoryTree) -> bool {
    state.commits.get(&tree.commit_id).is_some_and(|commit| {
        commit.kind == "directory_commit"
            && commit.provenance.as_deref() == Some(tree.tree_id.as_str())
            && state
                .directory_trees
                .get(&tree.tree_id)
                .is_some_and(|stored| stored.directory_branch_id == tree.directory_branch_id)
    })
}

/// The admitted State has an acyclic, complete parent graph. Working checkpoints
/// remain recovery evidence but are not user-visible directory-history nodes.
pub fn committed_directory_ancestor<'a>(
    state: &'a State,
    tree: &DirectoryTree,
) -> Option<&'a DirectoryTree> {
    let mut cursor = state.directory_trees.get(&tree.tree_id);
    while let Some(current) = cursor {
        if directory_tree_is_committed(state, current) {
            return Some(current);
        }
        cursor = current
            .parent_tree_id
            .as_ref()
            .and_then(|id| state.directory_trees.get(id));
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryStagingStatus {
    pub repository_id: String,
    pub directory_branch_id: String,
    pub head_tree_id: String,
    pub committed_tree_id: Option<String>,
    pub staged: bool,
    #[serialization(with = "crate::u64_string")]
    pub added: u64,
    #[serialization(with = "crate::u64_string")]
    pub modified: u64,
    #[serialization(with = "crate::u64_string")]
    pub deleted: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryCommitBody {
    pub expected_tree_id: String,
    #[serialization(with = "crate::u64_string")]
    pub expected_index_generation: u64,
    pub message: String,
    pub operation_id: crate::OperationId,
}
