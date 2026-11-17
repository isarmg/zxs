//! Automatic naming is an original intent, not a client-selected mount name.
//! Its resolved name belongs to the reservation/status, never a rewritten retry.
use crate::snapshot_worktree::{CreateWorktree, SnapshotHandle};
use crate::working_tree::DirectoryId;
use crate::{Generation, OperationId};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum WorktreeNaming {
    Automatic,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AllocateWorktree {
    pub source: SnapshotHandle,
    pub parent_directory_id: DirectoryId,
    pub expected_root_generation: Generation,
    pub operation_id: OperationId,
    pub naming: WorktreeNaming,
}

/// Both alternatives have strict, disjoint object shapes. Named preserves the
/// existing request bytes; Automatic requires naming and never accepts a name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(untagged)]
pub enum WorktreeCreationRequest {
    Named(CreateWorktree),
    Automatic(AllocateWorktree),
}
