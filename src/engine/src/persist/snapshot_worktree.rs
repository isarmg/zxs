//! Strict permanent reservation/completion records. Local material never supplies them.
use crate::directory_worktree::ledger::{Binding as LedgerBinding, Checkpoint, EditRecord};
use crate::working_tree::snapshot::{
    SnapshotEntry, SnapshotFile, SnapshotFileSource, SnapshotNodeBinding, SnapshotReference,
};
use crate::{Engine, OperationResult};
use data_serialization::{Deserialize, Serialize};
use storage_contract::snapshot_worktree::{
    CommitWorktree, CreateWorktree, EditWorktree, InitializeSnapshot, NamespaceHostAddress,
    WorktreeId,
    WorktreeRevision,
};
use storage_contract::working_tree::{
    DirectoryBinding, DiskRegistrationIdentity, LocalDirectoryBinding, LocalEntityIdentity,
    MaterialCommitment, SnapshotBranchId,
    SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{
    BranchName, CommitState, EntryId, OperationId, Principal, ProtectionAtPublish, VersionId,
};

pub(crate) const NODE_RESERVE: &str = "snapshot.repository.reserve";
pub(crate) const NODE_COMPLETE: &str = "snapshot.repository.complete";
pub(crate) const WORKTREE_RESERVE: &str = "snapshot.worktree.reserve";
pub(crate) const WORKTREE_ALLOCATE: &str = "snapshot.worktree.allocate";
pub(crate) const WORKTREE_COMPLETE: &str = "snapshot.worktree.complete";
pub(crate) const EDIT_RESERVE: &str = "snapshot.worktree.edit-reserve";
pub(crate) const EDIT_COMPLETE: &str = "snapshot.worktree.edit-complete";
pub(crate) const COMMIT_RESERVE: &str = "snapshot.worktree.commit-reserve";
pub(crate) const COMMIT_NODE: &str = "snapshot.worktree.commit-node";
pub(crate) const COMMIT_COMPLETE: &str = "snapshot.worktree.commit-complete";

/// A request-domain commitment, never a file content hash or write capability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(transparent)]
pub(crate) struct FileRequestCommitment(MaterialCommitment);
impl FileRequestCommitment {
    pub(crate) fn capture(workspace: &crate::Workspace, request: &crate::CommitRequest)
        -> Result<Self, CodecError>
    {
        let digest = crate::business::native_commit_digest(workspace, request)
            .map_err(CodecError::Request)?;
        let bytes: storage_contract::ContentHash = digest.parse().map_err(CodecError::Digest)?;
        Ok(Self(MaterialCommitment::from_digest(bytes.digest())))
    }

    pub(crate) fn matches(&self, digest: &str) -> bool {
        self.0.hash().to_string() == digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CommitReservation {
    pub(crate) request: CommitWorktree,
    pub(crate) source: storage_contract::snapshot_worktree::SnapshotHandle,
    pub(crate) binding: LedgerBinding,
    pub(crate) before: Checkpoint,
    pub(crate) edit: EditRecord,
    pub(crate) original: SnapshotFile,
    pub(crate) original_source: SnapshotFileSource,
    pub(crate) file_history: storage_contract::HistoryTreeId,
    pub(crate) file_base: storage_contract::CommitId,
    pub(crate) file_operation: OperationId,
    pub(crate) file_request: FileRequestCommitment,
    pub(crate) node: SnapshotNodeId,
    pub(crate) node_publication: OperationId,
    pub(crate) completion: OperationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CommitNode {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) binding: SnapshotNodeBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CommitCompletion {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) checkpoint: Checkpoint,
}

/// Recorded addressing evidence. Only the actual Engine root consumer can
/// compare it with its current registration and held host to acquire I/O scope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct HostEvidence {
    pub(crate) namespace: NamespaceHostAddress,
    pub(crate) registration: DiskRegistrationIdentity,
    pub(crate) root_entity: LocalEntityIdentity,
    pub(crate) common_data: LocalDirectoryBinding,
    /// Original common-root-to-host order, not a lookup inferred from a path.
    pub(crate) directories: Vec<NamespaceDirectoryBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NamespaceDirectoryBinding {
    pub(crate) entry: EntryId,
    pub(crate) directory: DirectoryBinding,
}

impl HostEvidence {
    pub(crate) fn host(&self) -> LocalDirectoryBinding {
        match self.directories.last() {
            Some(last) => LocalDirectoryBinding {
                directory_id: last.directory.directory_id,
                entity: last.directory.entity,
            },
            None => self.common_data.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NodeAllocation {
    pub(crate) repository: SnapshotRepositoryId,
    pub(crate) branch: SnapshotBranchId,
    pub(crate) node: SnapshotNodeId,
    pub(crate) completion: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NodeReservation {
    pub(crate) request: InitializeSnapshot,
    pub(crate) host: HostEvidence,
    pub(crate) allocated: NodeAllocation,
    pub(crate) branch_name: BranchName,
    /// Exact occurrence sources are captured by the same live namespace plan.
    /// Their full manifest is rebuilt by the one existing manifest constructor.
    pub(crate) entries: Vec<SnapshotEntry>,
    pub(crate) material: SnapshotReference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NodeCompletion {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) binding: SnapshotNodeBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct WorktreeAllocation {
    pub(crate) worktree: WorktreeId,
    pub(crate) completion: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct WorktreeReservation {
    pub(crate) request: CreateWorktree,
    pub(crate) host: HostEvidence,
    pub(crate) allocated: WorktreeAllocation,
    pub(crate) material: SnapshotReference,
}

/// The original automatic intent remains independent of the resolved plan.
/// The request digest covers request; the reservation commitment covers both.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AllocatedWorktreeReservation {
    pub(crate) request: storage_contract::snapshot_worktree::creation::AllocateWorktree,
    pub(crate) resolved: WorktreeReservation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct WorktreeCompletion {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) binding: LedgerBinding,
    pub(crate) checkpoint: Checkpoint,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct EditReservation {
    pub(crate) request: EditWorktree,
    pub(crate) binding: LedgerBinding,
    pub(crate) before: Checkpoint,
    pub(crate) original: SnapshotFile,
    pub(crate) original_source: SnapshotFileSource,
    pub(crate) version: VersionId,
    pub(crate) completion: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct EditCompletion {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) edit: EditRecord,
    pub(crate) checkpoint: Checkpoint,
    pub(crate) revision: WorktreeRevision,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "record", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Record {
    ReserveNode(NodeReservation),
    CompleteNode(NodeCompletion),
    ReserveWorktree(WorktreeReservation),
    ReserveAllocatedWorktree(AllocatedWorktreeReservation),
    CompleteWorktree(WorktreeCompletion),
    ReserveEdit(EditReservation),
    CompleteEdit(EditCompletion),
    ReserveCommit(CommitReservation),
    PublishCommitNode(CommitNode),
    CompleteCommit(CommitCompletion),
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    snapshot_worktree: Record,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: Vec<()>,
}

pub(crate) struct Decoded {
    pub(crate) record: Record,
    pub(crate) timestamp: u64,
    pub(crate) membership_epoch: u64,
}

#[derive(Debug, error_definition::Error)]
pub enum CodecError {
    #[error("snapshot/worktree receipt is not an exact ordinary metadata publication")]
    Publication,
    #[error("snapshot/worktree receipt operation kind and typed record differ")]
    Kind,
    #[error("snapshot/worktree canonical record encoding failed")]
    Encoding(#[source] storage_contract::Error),
    #[error("snapshot/worktree request or result encoding failed")]
    Value(#[source] json_codec::Error),
    #[error("snapshot/worktree receipt decoding failed")]
    Decoding(#[source] json_codec::Error),
    #[error("snapshot/worktree original request digest failed")]
    Request(#[source] crate::Error),
    #[error("snapshot/worktree receipt exceeds the manifest entry bound")]
    Budget,
    #[error("decoding the ordinary FILE publication request commitment")]
    Digest(#[source] storage_contract::ParseContentHashError),
}

impl Record {
    fn request(&self) -> Result<json_codec::Value, CodecError> {
        match self {
            Self::ReserveNode(record) => PublicationEncoding::node_request(&record.request),
            Self::ReserveWorktree(record) => {
                json_codec::to_value(&record.request).map_err(CodecError::Value)
            }
            Self::ReserveAllocatedWorktree(record) => {
                json_codec::to_value(&record.request).map_err(CodecError::Value)
            }
            Self::ReserveEdit(record) => {
                json_codec::to_value(&record.request).map_err(CodecError::Value)
            }
            Self::ReserveCommit(record) => {
                json_codec::to_value(&record.request).map_err(CodecError::Value)
            }
            Self::CompleteNode(_) | Self::CompleteWorktree(_) | Self::CompleteEdit(_)
            | Self::PublishCommitNode(_) | Self::CompleteCommit(_) => {
                json_codec::to_value(self).map_err(CodecError::Value)
            }
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::ReserveNode(_) => NODE_RESERVE,
            Self::CompleteNode(_) => NODE_COMPLETE,
            Self::ReserveWorktree(_) => WORKTREE_RESERVE,
            Self::ReserveAllocatedWorktree(_) => WORKTREE_ALLOCATE,
            Self::CompleteWorktree(_) => WORKTREE_COMPLETE,
            Self::ReserveEdit(_) => EDIT_RESERVE,
            Self::CompleteEdit(_) => EDIT_COMPLETE,
            Self::ReserveCommit(_) => COMMIT_RESERVE,
            Self::PublishCommitNode(_) => COMMIT_NODE,
            Self::CompleteCommit(_) => COMMIT_COMPLETE,
        }
    }

    pub(crate) fn commitment(&self) -> Result<MaterialCommitment, CodecError> {
        let bytes = storage_contract::canonical_json(self).map_err(CodecError::Encoding)?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"zxs:snapshot-worktree-reservation\0");
        hash.update(&bytes);
        Ok(MaterialCommitment::from_digest(hash.finalize()))
    }

    pub(crate) fn digest(&self, principal: &Principal) -> Result<String, CodecError> {
        let input = self.request()?;
        Engine::native_request_digest(self.kind(), principal.as_str(), &input)
            .map_err(CodecError::Request)
    }

    /// The ordinary metadata planner adds the existing publication footer.
    pub(crate) fn result(&self) -> Result<json_codec::Value, CodecError> {
        #[derive(Serialize)]
        struct ResultRecord<'a> {
            snapshot_worktree: &'a Record,
        }
        json_codec::to_value(ResultRecord { snapshot_worktree: self }).map_err(CodecError::Value)
    }
}

/// The existing mutation boundary still accepts encoded request/result values.
/// Keep that representation here; the live lifecycle never inspects its fields.
pub(crate) struct PublicationEncoding {
    request: json_codec::Value,
    result: json_codec::Value,
}

impl PublicationEncoding {
    pub(crate) fn node_request(request: &InitializeSnapshot)
        -> Result<json_codec::Value, CodecError>
    {
        json_codec::to_value(request).map_err(CodecError::Value)
    }

    pub(crate) fn new(record: &Record) -> Result<Self, CodecError> {
        Ok(Self { request: record.request()?, result: record.result()? })
    }

    pub(crate) fn request(&self) -> json_codec::Value {
        self.request.clone()
    }

    pub(crate) fn result(&self) -> json_codec::Value {
        self.result.clone()
    }
}

fn indexed_kind(kind: &str) -> bool {
    matches!(kind,
        NODE_RESERVE | NODE_COMPLETE | WORKTREE_RESERVE | WORKTREE_ALLOCATE | WORKTREE_COMPLETE
            | EDIT_RESERVE | EDIT_COMPLETE | COMMIT_RESERVE | COMMIT_NODE | COMMIT_COMPLETE)
}

/// A malformed record alias also needs its actual original before indexing.
/// Classification alone is not decoding, admission or publication evidence.
pub(crate) fn requires_original(receipt: &OperationResult) -> bool {
    indexed_kind(&receipt.kind) || receipt.result.get("snapshot_worktree").is_some()
}

/// This decoder handles the original ordinary receipt, not an unverified
/// recovery overlay. Degraded publication support must supply its authenticated
/// original/outcome pair at the real recovery boundary rather than ignore keys.
pub(crate) fn decode(receipt: &OperationResult) -> Result<Option<Decoded>, CodecError> {
    let selected = indexed_kind(&receipt.kind);
    if !selected && receipt.result.get("snapshot_worktree").is_none() {
        return Ok(None);
    }
    if !selected {
        return Err(CodecError::Kind);
    }
    if receipt.commit_state != CommitState::Published
        || receipt.protection_at_publish != ProtectionAtPublish::NotApplicable
        || receipt.commit_id.is_some()
        || receipt.history_tree_id.is_some()
        || receipt.branch_id.is_some()
    {
        return Err(CodecError::Publication);
    }
    // Bound the array before cloning and typed decoding this already bounded
    // metadata value. The same manifest constructor owns all further budgets.
    let record = receipt.result.get("snapshot_worktree")
        .and_then(|envelope| envelope.get("record"));
    if record.and_then(|record| record.get("entries"))
        .and_then(json_codec::Value::as_array)
        .is_some_and(|entries| entries.len() > crate::working_tree::snapshot::MAX_ENTRIES)
    {
        return Err(CodecError::Budget);
    }
    // Check both recognized shapes, including a malformed object containing
    // both. Unknown fields must not hide an oversized nested host before clone.
    let hosts = [
        record.and_then(|record| record.get("host")),
        record.and_then(|record| record.get("resolved")).and_then(|plan| plan.get("host")),
    ];
    if hosts.into_iter().flatten().any(|host| {
        host.get("directories").and_then(json_codec::Value::as_array)
            .is_some_and(|directories| {
                directories.len() > crate::working_tree::admission::MAX_DIRECTORY_DEPTH
            })
    }) {
        return Err(CodecError::Budget);
    }
    let envelope: Envelope = json_codec::from_value(receipt.result.clone())
        .map_err(CodecError::Decoding)?;
    if envelope.snapshot_worktree.kind() != receipt.kind {
        return Err(CodecError::Kind);
    }
    if !envelope.version_protection.is_empty() {
        return Err(CodecError::Publication);
    }
    Ok(Some(Decoded {
        record: envelope.snapshot_worktree,
        timestamp: envelope.timestamp,
        membership_epoch: envelope.membership_epoch,
    }))
}
