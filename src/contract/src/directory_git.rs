//! Explicit directory repositories. Working compositions and committed HEADs
//! are distinct; a parent index pins only explicitly staged child commits.
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod visibility;
pub use visibility::current_entry_visible;

/// The exact initialized source reviewed before creating a directory branch.
/// This is a wire request value; current ownership and state are checked at commit.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryBranchCreateSource {
    pub repository_id: String,
    pub directory_id: String,
    pub directory_branch_id: String,
    pub working_tree_id: String,
    pub head_tree_id: String,
    #[serialization(with = "crate::u64_string")]
    pub index_generation: u64,
    pub namespace_head: String,
}

impl DirectoryBranchCreateSource {
    pub fn validate(&self) -> crate::Result<()> {
        let Self {
            repository_id,
            directory_id,
            directory_branch_id,
            working_tree_id,
            head_tree_id,
            index_generation,
            namespace_head,
        } = self;
        crate::validate_id(repository_id, "repo")?;
        if directory_id != repository_id {
            crate::validate_id(directory_id, "entry")?;
        }
        crate::validate_id(directory_branch_id, "dbr")?;
        crate::validate_id(working_tree_id, "dtree")?;
        crate::validate_id(head_tree_id, "dtree")?;
        crate::validate_id(namespace_head, "commit")?;
        if *index_generation == 0 {
            return Err(crate::Error::new(
                crate::ErrorCode::InvalidInteger,
                "directory source index generation must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitChildReference {
    pub directory_branch_id: String,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub tree_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryGitBranch {
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
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
    /// Active reversible directory containers owned by this repository.
    pub deletions: BTreeMap<String, DirectoryDeletionNode>,
}

/// A file already deleted before the directory remains deleted after restoring
/// the directory. Only members created by this exact directory operation undo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryDeletionMember {
    pub deletion_node_id: String,
    pub created_by_directory: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryDeletionNode {
    pub node_id: String,
    pub repository_id: String,
    pub history_tree_id: String,
    pub owner_directory_id: String,
    pub directory_id: String,
    pub path: name_policy::PortablePath,
    pub namespace_head: String,
    pub directory_branch_id: String,
    pub source_tree_id: String,
    pub operation_id: crate::OperationId,
    pub author: String,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub created_at: u64,
    pub files: BTreeMap<String, DirectoryDeletionMember>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryDeletionSummary {
    pub directory_id: String,
    pub deletion_node_id: String,
    pub path: name_policy::PortablePath,
    pub operation_id: crate::OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryDeletionsPage {
    pub snapshot: String,
    pub entries: Vec<DirectoryDeletionSummary>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct UndoDirectoryDeletionBody {
    pub owner_directory_id: String,
    pub directory_id: String,
    pub deletion_node_id: String,
    pub operation_id: crate::OperationId,
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
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub head_tree_id: Option<String>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub staged_tree_id: Option<String>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
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
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub owner: Option<DirectoryGitOwner>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub directory_branch_id: Option<String>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub working_tree_id: Option<String>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
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

/// Explicit current-state repair. The outer namespace head is a location guard,
/// not a frozen preview of every independent child's stable main.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryLayoutReconcileBody {
    pub expected_namespace_head: String,
}

/// Structural placement observed at execution, not a content-verification or
/// durable business-operation receipt. Unavailable members remain explicit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryLayoutResult {
    pub repository_id: String,
    #[serialization(with = "crate::u64_string")]
    pub state_sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub directories: u64,
    #[serialization(with = "crate::u64_string")]
    pub containers: u64,
    #[serialization(with = "crate::u64_string")]
    pub unavailable_containers: u64,
    #[serialization(with = "crate::u64_string")]
    pub unavailable_histories: u64,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/directory_git.rs"]
mod tests;
