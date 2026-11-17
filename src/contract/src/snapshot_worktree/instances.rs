//! A complete bounded set of admitted mounts, never a truncated discovery result.
use crate::RepositoryId;
use crate::snapshot_worktree::{SnapshotRootContext, WorktreeStatus};
use data_serialization::{Deserialize, Serialize};
use std::num::NonZeroU16;

pub const DIRECTORY_WORKTREE_LIST_LIMIT: NonZeroU16 =
    NonZeroU16::new(100).expect("invariant: the literal management list bound is nonzero");

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryWorktrees {
    pub repository_id: RepositoryId,
    pub context: SnapshotRootContext,
    pub instances: Vec<WorktreeStatus>,
}
