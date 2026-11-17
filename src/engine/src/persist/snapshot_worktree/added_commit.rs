//! One Added occurrence, anchored before its actual first FILE preparation.
use crate::business::file_draft::PublishedContent;
use crate::directory_worktree::ledger::{AddedFile, Binding, Checkpoint};
use crate::persist::layout::container::WorkingFullId;
use crate::working_tree::snapshot::SnapshotFile;
use data_serialization::{Deserialize, Serialize};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle};
use storage_contract::working_tree::{
    LocalEntityIdentity, MaterialCommitment, SnapshotBranchId, SnapshotNodeId,
};
use storage_contract::{BranchId, BranchName, CommitId, HistoryTreeId, OperationId, TreeId};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AddedCommit {
    pub(crate) request: CommitWorktree,
    pub(crate) source: SnapshotHandle,
    pub(crate) observed_head: SnapshotHandle,
    pub(crate) destination: SnapshotBranchId,
    pub(crate) branch_name: BranchName,
    pub(crate) binding: Binding,
    pub(crate) before: Checkpoint,
    pub(crate) added: AddedFile,
    pub(crate) history: HistoryTreeId,
    pub(crate) file_branch: BranchId,
    pub(crate) file_commit: CommitId,
    pub(crate) file_tree: TreeId,
    pub(crate) file_operation: OperationId,
    pub(crate) node: SnapshotNodeId,
    pub(crate) node_publication: OperationId,
    pub(crate) completion: OperationId,
}
impl AddedCommit {
    pub(crate) fn routing(&self) -> super::CommitRouting {
        use crate::directory_worktree::lifecycle::DirectoryDestination;
        super::CommitRouting {
            directory: if self.source == self.observed_head {
                DirectoryDestination::Continue {
                    branch: self.destination,
                    expected_head: self.source.node_id,
                }
            } else {
                DirectoryDestination::Fork {
                    branch: self.destination,
                    expected_source_head: self.observed_head.node_id,
                }
            },
            file_branch: self.file_branch,
        }
    }
    pub(crate) fn handle(&self) -> SnapshotHandle {
        SnapshotHandle {
            publication: self.node_publication,
            repository_id: self.source.repository_id,
            branch_id: self.destination,
            node_id: self.node,
        }
    }
}

/// Descriptive actual preparation facts. These never reopen or issue a hot FULL.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AddedFull {
    pub(crate) id: WorkingFullId,
    pub(crate) entity: LocalEntityIdentity,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AddedFirst {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) content: PublishedContent,
    pub(crate) full: AddedFull,
}
impl AddedFirst {
    pub(crate) fn material(&self) -> Result<SnapshotFile, storage_contract::Error> {
        Ok(SnapshotFile {
            file_id: self.content.version.file_id.parse()?,
            version_id: self.content.version.version_id.parse()?,
            size: self.content.version.logical_size,
            content_hash: self.content.version.content_hash,
        })
    }
}

pub(crate) fn first_request(
    reserve: OperationId,
    reservation: MaterialCommitment,
) -> crate::Result<json_codec::Value> {
    #[derive(Serialize)]
    struct Request {
        reserve: OperationId,
        reservation: MaterialCommitment,
    }
    json_codec::to_value(Request {
        reserve,
        reservation,
    })
    .map_err(|source| {
        crate::business::file_draft::worktree::failure(
            crate::snapshot_worktree::RegistrationError::Codec(super::CodecError::Value(source)),
        )
    })
}
