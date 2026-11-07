//! Explicit directory repositories. Working compositions and committed HEADs
//! are distinct; a parent index pins only explicitly staged child commits.
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitChildReference {
    pub directory_branch_id: String,
    #[serialization(deserialize_with = "required_nullable")]
    pub tree_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitBranch {
    #[serialization(deserialize_with = "required_nullable")]
    pub head_tree_id: Option<String>,
    pub index_tree_id: String,
    #[serialization(with = "crate::u64_string")]
    pub index_generation: u64,
    /// Direct repository boundaries reviewed when this index was last staged.
    pub index_boundaries: BTreeMap<String, String>,
    pub index_children: BTreeMap<String, DirectoryGitChildReference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitRepository {
    pub repository_id: String,
    pub directory_id: String,
    #[serialization(with = "crate::u64_string")]
    pub initialized_sequence: u64,
    /// Stable directory branch whose saved workspace defines the main layout.
    /// Renaming or checking out another branch does not change this identity.
    pub main_branch_id: String,
    pub current_branch_id: String,
    pub branches: BTreeMap<String, DirectoryGitBranch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitOwner {
    pub directory_id: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitChildStatus {
    pub directory_id: String,
    pub path: String,
    pub directory_branch_id: String,
    #[serialization(deserialize_with = "required_nullable")]
    pub head_tree_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    pub staged_tree_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    pub committed_tree_id: Option<String>,
    pub dirty: bool,
    pub reference_changed: bool,
    pub staged: bool,
    pub needs_restage: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitStatus {
    pub repository_id: String,
    pub directory_id: String,
    pub path: String,
    pub initialized: bool,
    #[serialization(deserialize_with = "required_nullable")]
    pub owner: Option<DirectoryGitOwner>,
    #[serialization(deserialize_with = "required_nullable")]
    pub directory_branch_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    pub working_tree_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    pub head_tree_id: Option<String>,
    #[serialization(with = "crate::u64_string")]
    pub index_generation: u64,
    pub staged: bool,
    /// Owned working contents differ from the explicitly reviewed index.
    pub unstaged: bool,
    pub needs_restage: bool,
    #[serialization(with = "crate::u64_string")]
    pub added: u64,
    #[serialization(with = "crate::u64_string")]
    pub modified: u64,
    #[serialization(with = "crate::u64_string")]
    pub deleted: u64,
    pub children: Vec<DirectoryGitChildStatus>,
    pub legacy_history: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitInitializeBody {
    pub path: String,
    pub expected_namespace_head: String,
    pub operation_id: crate::OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitStageChildBody {
    pub directory_id: String,
    pub directory_branch_id: String,
    #[serialization(with = "crate::u64_string")]
    pub expected_index_generation: u64,
    pub child_directory_id: String,
    pub expected_child_head_tree_id: String,
    pub operation_id: crate::OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitStageOwnedBody {
    pub directory_id: String,
    pub directory_branch_id: String,
    pub expected_tree_id: String,
    #[serialization(with = "crate::u64_string")]
    pub expected_index_generation: u64,
    pub operation_id: crate::OperationId,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: data_serialization::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/directory_git.rs"]
mod tests;
