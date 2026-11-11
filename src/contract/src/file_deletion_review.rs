//! Bounded read projections of immutable file-deletion audit records.
use crate::{
    BranchId, FileDeletionId, FileId, FileNodeId, HistoryTreeId, OperationId, Principal,
    RepositoryId, Sequence, Timestamp,
};
use data_serialization::{Deserialize, Deserializer, Serialize};
use name_policy::PortablePath;

pub const PAGE_SIZE: usize = 100;
pub const MAX_RESPONSE_BYTES: usize = crate::client_protocol::MAX_FILE_DELETION_RESPONSE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Summary {
    pub file_id: FileId,
    pub node_id: FileDeletionId,
    pub repository_id: RepositoryId,
    pub history_tree_id: HistoryTreeId,
    pub path: PortablePath,
    pub operation_id: OperationId,
    pub author: Principal,
    pub sequence: Sequence,
    pub created_at: Timestamp,
    #[serialization(with = "crate::u64_string")]
    pub branch_count: u64,
    #[serialization(with = "crate::u64_string")]
    pub parent_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BranchHead {
    pub branch_id: BranchId,
    pub node_id: FileNodeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BranchPage {
    pub file_id: FileId,
    pub deletion_node_id: FileDeletionId,
    pub entries: Vec<BranchHead>,
    #[serialization(deserialize_with = "nullable")]
    pub next_after: Option<BranchId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ParentPage {
    pub file_id: FileId,
    pub deletion_node_id: FileDeletionId,
    #[serialization(with = "crate::u64_string")]
    pub offset: u64,
    pub entries: Vec<FileNodeId>,
    #[serialization(with = "crate::optional_u64_string")]
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct UndoBody {
    pub deletion_node_id: FileDeletionId,
    pub operation_id: OperationId,
}

fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/file_deletion_review.rs"]
mod tests;
