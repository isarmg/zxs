//! Canonical owner-approved records for continuing one registered directory branch.
//! These fields never grant permission to use a path or move an inode.
use crate::business::file_draft::{Allocation, PublishedContent};
use crate::directory_worktree::ledger::{Binding, Checkpoint, EditRecord};
use crate::directory_worktree::projection::FileResult;
use crate::directory_worktree::store::SparseWorktreeBinding;
use crate::persist::snapshot_worktree::HostEvidence;
use crate::working_tree::snapshot::{SnapshotEntry, SnapshotFile, SnapshotNodeBinding};
use data_serialization::{Deserialize, Serialize};
use name_policy::PortablePath;
use storage_contract::{CommitId, DiskId, Generation, HistoryTreeId, OperationId, TreeId};
use storage_contract::file_container::{ContainerEntity, ContainerNamespace};
use storage_contract::file_draft::{DraftBase, FileWorkingState};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle, SnapshotHost};
use storage_contract::working_tree::{DirectoryId, MaterialCommitment, RootId, SnapshotNodeId};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub(crate) edit_operation: OperationId,
    pub(crate) edit_completion: OperationId,
    pub(crate) edit: EditRecord,
    pub(crate) original: SnapshotFile,
    pub(crate) base: DraftBase,
    pub(crate) owner: DiskId,
    pub(crate) namespace: ContainerNamespace,
    pub(crate) owner_root: RootId,
    pub(crate) owner_entity: ContainerEntity,
    pub(crate) history: HistoryTreeId,
    pub(crate) branch_base: CommitId,
    pub(crate) clean: FileWorkingState,
    pub(crate) commit: CommitId,
    pub(crate) tree: TreeId,
    pub(crate) allocations: Vec<Allocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Reservation {
    pub(crate) request: CommitWorktree,
    pub(crate) source: SnapshotHandle,
    pub(crate) source_binding: SnapshotNodeBinding,
    pub(crate) host: HostEvidence,
    pub(crate) root: SnapshotHost,
    pub(crate) ordinary_head: CommitId,
    pub(crate) ordinary_generation: Generation,
    pub(crate) instance: SparseWorktreeBinding,
    pub(crate) ledger: Binding,
    pub(crate) before: Checkpoint,
    pub(crate) destination_node: SnapshotNodeId,
    pub(crate) publish_operation: OperationId,
    pub(crate) rows: Vec<Input>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PreparedNode {
    pub(crate) preparation: DirectoryId,
    pub(crate) binding: SnapshotNodeBinding,
    pub(crate) entries: Vec<SnapshotEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Output {
    pub(crate) path: PortablePath,
    pub(crate) commit: CommitId,
    pub(crate) tree: TreeId,
    pub(crate) file: FileResult,
    pub(crate) content: PublishedContent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Publication {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) prepared_node: PreparedNode,
    pub(crate) rows: Vec<Output>,
    pub(crate) committing: Checkpoint,
    pub(crate) completed: Checkpoint,
}
