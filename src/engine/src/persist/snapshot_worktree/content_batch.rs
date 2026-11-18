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

pub(super) fn publish_request(reserve: OperationId, reservation: MaterialCommitment)
    -> Result<json_codec::Value, super::CodecError>
{
    #[derive(Serialize)]
    struct Request { reserve: OperationId, reservation: MaterialCommitment }
    json_codec::to_value(Request { reserve, reservation }).map_err(super::CodecError::Value)
}

pub(super) fn require_shape(receipt: &crate::OperationResult, record: &json_codec::Value)
    -> Result<(), super::CodecError>
{
    use crate::working_tree::snapshot::{MAX_ENTRIES, MAX_VERSIONS};
    use super::CodecError;
    let rows = record.get("rows").and_then(json_codec::Value::as_array).ok_or(CodecError::Budget)?;
    if rows.is_empty() || rows.len() > MAX_VERSIONS { return Err(CodecError::Budget); }
    let publishing = receipt.kind == super::CONTENT_PUBLISH;
    let protection = receipt.result.get("version_protection")
        .and_then(json_codec::Value::as_array).ok_or(CodecError::Publication)?;
    if protection.len() != if publishing { rows.len() } else { 0 } {
        return Err(CodecError::Publication);
    }
    if publishing && record.get("prepared_node").and_then(|node| node.get("entries"))
        .and_then(json_codec::Value::as_array).is_none_or(|entries| entries.len() > MAX_ENTRIES)
    { return Err(CodecError::Budget); }
    // A lazy ordinary-root branch is materialized at most once in the one
    // accumulating projection. Each FILE row additionally owns its fixed tail.
    let mut total = 0usize;
    for (i, row) in rows.iter().enumerate() {
        let body = if publishing { row.get("content").ok_or(CodecError::Kind)? } else { row };
        let count = body.get("allocations").and_then(json_codec::Value::as_array)
            .ok_or(CodecError::Budget)?.len();
        total = total.checked_add(count).ok_or(CodecError::Budget)?;
        if count < 2 || count > MAX_ENTRIES * 3 + 8 || total > MAX_ENTRIES * 3 + rows.len() * 8 {
            return Err(CodecError::Budget);
        }
        if publishing {
            let version = body.get("version").ok_or(CodecError::Kind)?;
            let placement = body.get("placement").ok_or(CodecError::Kind)?;
            if version.get("source_version_ids").and_then(json_codec::Value::as_array)
                .is_none_or(|ids| ids.len() != 1)
                || placement.get("route_slot_ids").and_then(json_codec::Value::as_array)
                    .is_none_or(|ids| ids.len() != 1)
                || placement.get("binding_generations").and_then(json_codec::Value::as_object)
                    .is_none_or(|bindings| bindings.len() != 1)
                || ["required_slots", "slots"].iter().any(|key|
                    protection[i].get(*key).and_then(json_codec::Value::as_array)
                        .is_none_or(|slots| slots.len() != 1))
            { return Err(CodecError::Budget); }
        }
    }
    Ok(())
}

impl Reservation {
    pub(crate) fn handle(&self) -> SnapshotHandle {
        SnapshotHandle { publication: self.publish_operation,
            repository_id: self.source.repository_id, branch_id: self.source.branch_id,
            node_id: self.destination_node }
    }

    pub(crate) fn ledger_publication(&self) -> crate::directory_worktree::lifecycle::Publication {
        use crate::directory_worktree::lifecycle::{
            DirectoryDestination, Publication, PublicationIntent, PublicationMode, PublicationScope, SnapshotSource,
        };
        Publication {
            intent: PublicationIntent {
                worktree: self.request.target.worktree_id,
                source: SnapshotSource { repository: self.source.repository_id, node: self.source.node_id,
                    branch: self.source.branch_id, manifest: self.source_binding.reference().manifest },
                revision: self.request.expected, operation: self.request.operation_id,
                mode: PublicationMode::DirectoryCommit { destination: DirectoryDestination::Continue {
                    branch: self.source.branch_id, expected_head: self.source.node_id,
                }},
            },
            scope: PublicationScope::Directory { branch: self.source.branch_id, node: self.destination_node },
        }
    }

    pub(crate) fn events(&self, rows: &[Output]) -> [crate::directory_worktree::ledger::Event; 2] {
        use crate::directory_worktree::ledger::Event;
        [Event::Commit { operation: self.request.operation_id, expected: self.request.expected,
            current_head: self.source.node_id, fork_branch: self.source.branch_id },
         Event::Completed { publication: self.ledger_publication(),
            files: rows.iter().map(|row| row.file.clone()).collect() }]
    }
}
