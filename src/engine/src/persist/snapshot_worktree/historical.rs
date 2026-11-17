//! The original operation precedes a historical FILE branch or workspace.
//! Ready binds the actual existing FILE writer's precise command afterwards.
use super::{CodecError, CommitReservation};
use crate::directory_worktree::ledger::{Binding, Checkpoint, EditRecord};
use crate::directory_worktree::lifecycle::DirectoryDestination;
use crate::working_tree::snapshot::{SnapshotFile, SnapshotFileSource};
use crate::{Engine, OperationResult};
use data_serialization::{Deserialize, Serialize};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle};
use storage_contract::working_tree::{MaterialCommitment, SnapshotBranchId, SnapshotNodeId};
use storage_contract::{BranchId, BranchName, CommitId, HistoryTreeId, OperationId, Principal};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum HistoricalFile {
    Continue {
        branch: BranchId,
        base: CommitId,
    },
    Fork {
        operation: OperationId,
        name: BranchName,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct HistoricalCommit {
    pub(crate) request: CommitWorktree,
    pub(crate) source: SnapshotHandle,
    pub(crate) observed_head: SnapshotHandle,
    pub(crate) destination: SnapshotBranchId,
    pub(crate) branch_name: BranchName,
    pub(crate) binding: Binding,
    pub(crate) before: Checkpoint,
    pub(crate) edit: EditRecord,
    pub(crate) original: SnapshotFile,
    pub(crate) original_source: SnapshotFileSource,
    pub(crate) file_history: HistoryTreeId,
    pub(crate) file_observed_head: storage_contract::FileNodeId,
    pub(crate) file: HistoricalFile,
    pub(crate) ready: OperationId,
    pub(crate) file_operation: OperationId,
    pub(crate) node: SnapshotNodeId,
    pub(crate) node_publication: OperationId,
    pub(crate) completion: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct HistoricalReady {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) plan: CommitReservation,
    pub(crate) file_branch: BranchId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CommitRouting {
    pub(crate) directory: DirectoryDestination,
    pub(crate) file_branch: BranchId,
}

impl HistoricalCommit {
    pub(crate) fn directory(&self) -> DirectoryDestination {
        if self.source == self.observed_head {
            DirectoryDestination::Continue {
                branch: self.destination,
                expected_head: self.source.node_id,
            }
        } else {
            DirectoryDestination::Fork {
                branch: self.destination,
                expected_source_head: self.observed_head.node_id,
            }
        }
    }

    pub(crate) fn matches(&self, plan: &CommitReservation) -> bool {
        self.request == plan.request
            && self.source == plan.source
            && self.binding == plan.binding
            && self.before == plan.before
            && self.edit == plan.edit
            && self.original == plan.original
            && self.original_source == plan.original_source
            && self.file_history == plan.file_history
            && self.file_operation == plan.file_operation
            && self.node == plan.node
            && self.node_publication == plan.node_publication
            && self.completion == plan.completion
    }
}

/// Decode the existing file.branch.create receipt at the persist boundary.
/// The caller additionally binds these IDs to the actual branch/commit records.
#[derive(Deserialize)]
pub(crate) struct CreatedFileBranch {
    pub(crate) branch_id: BranchId,
    pub(crate) commit_id: CommitId,
}

pub(crate) fn file_branch(
    record: &OperationResult,
    original: &HistoricalCommit,
    principal: &Principal,
) -> Result<Option<CreatedFileBranch>, CodecError> {
    let HistoricalFile::Fork { operation, name } = &original.file else {
        return Ok(None);
    };
    let SnapshotFileSource::Container { node, .. } = original.original_source else {
        return Err(CodecError::Kind);
    };
    let body = json_codec::json!({
        "file_id": original.original.file_id.to_string(),
        "source_version_id": original.original.version_id.to_string(),
        "source_node_id": node.to_string(), "name": name.as_str(),
    });
    let digest = Engine::native_request_digest("file.branch.create", principal.as_str(), &body)
        .map_err(CodecError::Request)?;
    if record.operation_id != *operation
        || record.kind != "file.branch.create"
        || record.request_digest != digest
        || record.commit_state != storage_contract::CommitState::Published
    {
        return Err(CodecError::Kind);
    }
    json_codec::from_value(record.result.clone())
        .map(Some)
        .map_err(CodecError::Decoding)
}
