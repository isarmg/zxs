//! Exact instance-owned files, before their first historical version exists.
use crate::snapshot_worktree::{WorktreeHandle, WorktreeRevision};
use crate::{EntryId, FileId, OperationId};
use data_serialization::{Deserialize, Serialize};
use name_policy::PortablePath;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CreateWorktreeFile {
    pub target: WorktreeHandle,
    pub path: PortablePath,
    pub expected: WorktreeRevision,
    pub operation_id: OperationId,
}

/// The path is descriptive; the original birth and stable occurrence remain
/// authoritative even when another instance contains the same visible spelling.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorktreeFileTarget {
    pub worktree: WorktreeHandle,
    pub path: PortablePath,
    pub entry: EntryId,
    pub file: FileId,
}
