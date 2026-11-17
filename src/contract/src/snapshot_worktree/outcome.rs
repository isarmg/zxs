//! Read-only original-operation results. Reservation operation IDs are not
//! completion or node-publication IDs. Pending never grants a retry or cleanup.
use crate::RepositoryId;
use crate::snapshot_worktree::{CommitWorktree, CreateWorktree, SnapshotHandle, WorktreeHandle};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorktreeCreationOutcome {
    Pending {},
    Complete { target: WorktreeHandle },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorktreeCreationReceipt {
    pub repository_id: RepositoryId,
    pub request: CreateWorktree,
    pub result: WorktreeCreationOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorktreeCommitOutcome {
    Pending {},
    Complete { snapshot: SnapshotHandle },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorktreeCommitReceipt {
    pub repository_id: RepositoryId,
    pub request: CommitWorktree,
    pub result: WorktreeCommitOutcome,
}
