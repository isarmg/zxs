//! Exact native FILE birth/save intents and their independent instance tail.
use super::*;
use storage_contract::file_container::retained::RetainedAllocation;
use storage_contract::file_draft::manifest::WorkingManifest;
use storage_contract::file_draft::{UnbornWorking, WorkingRevision};
use storage_contract::snapshot_worktree::additions::{CreateWorktreeFile, WorktreeFileTarget};
use storage_contract::{BranchId, DiskId, HistoryTreeId, RepositoryId};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct WorktreeFileBirth {
    pub(crate) request: CreateWorktreeFile,
    pub(crate) target: WorktreeFileTarget,
    pub(crate) repository: RepositoryId,
    pub(crate) branch: BranchId,
    pub(crate) history: HistoryTreeId,
    pub(crate) owner: DiskId,
    pub(crate) allocation: RetainedAllocation,
    pub(crate) binding: LedgerBinding,
    pub(crate) before: Checkpoint,
    pub(crate) completion: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct WorktreeFileSaveRequest {
    pub(crate) target: WorktreeFileTarget,
    pub(crate) expected: WorktreeRevision,
    pub(crate) working: WorkingRevision,
    pub(crate) manifest: WorkingManifest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct WorktreeFileSave {
    pub(crate) request: WorktreeFileSaveRequest,
    pub(crate) birth: OperationId,
    pub(crate) binding: LedgerBinding,
    pub(crate) before: Checkpoint,
    pub(crate) completion: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct WorktreeFileCompletion {
    pub(crate) original: OperationId,
    pub(crate) commitment: MaterialCommitment,
    pub(crate) working: UnbornWorking,
    pub(crate) checkpoint: Checkpoint,
    pub(crate) revision: WorktreeRevision,
}
