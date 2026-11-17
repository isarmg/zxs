//! Published formal snapshot nodes, not names or mutable namespace selections.
//! Pagination binds an exact admitted SYSTEM sequence and a complete handle.
use crate::snapshot_worktree::{SnapshotHandle, SnapshotRootContext};
use crate::{BranchName, RepositoryId, Sequence};
use data_serialization::{Deserialize, Serialize};

pub const SNAPSHOT_HISTORY_PAGE_SIZE: usize = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SnapshotHistoryCursor {
    pub sequence: Sequence,
    pub after: SnapshotHandle,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SnapshotHistoryNode {
    pub snapshot: SnapshotHandle,
    pub branch_name: BranchName,
    pub parent: Option<SnapshotHandle>,
    pub is_head: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectorySnapshotHistory {
    pub repository_id: RepositoryId,
    pub context: SnapshotRootContext,
    pub nodes: Vec<SnapshotHistoryNode>,
    pub next_cursor: Option<SnapshotHistoryCursor>,
}
