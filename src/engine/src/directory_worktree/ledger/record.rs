use super::{EditLeaf, LedgerError};
use crate::directory_worktree::lifecycle::{Mount, Publication, Revision, SnapshotSource};
use crate::directory_worktree::projection::{FileEdit, FileResult, WorkingFile};
use crate::directory_worktree::projection::namespace::NamespaceChange;
use crate::directory_worktree::store::SparseWorktreeBinding;
use crate::working_tree::snapshot::{SnapshotFile, SnapshotFileSource};
use data_serialization::{Deserialize, Deserializer, Serialize};
use name_policy::PortablePath;
use storage_contract::{OperationId, Sequence};
use storage_contract::snapshot_worktree::additions::WorktreeFileTarget;
use storage_contract::file_draft::UnbornWorking;
use storage_contract::working_tree::{
    LocalEntityIdentity, MaterialCommitment, SnapshotBranchId, SnapshotNodeId,
};

/// This records a completed write with a frozen exact inode, not an open upload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct EditRecord {
    /// Exact original snapshot occurrence, not its current visible path. This
    /// also anchors the physical leaf after logical rename or removal.
    pub(crate) path: PortablePath,
    pub(crate) material: SnapshotFile,
    pub(crate) leaf: EditLeaf,
    pub(crate) entity: LocalEntityIdentity,
}
impl EditRecord {
    pub(crate) fn file_edit(&self) -> FileEdit {
        FileEdit {
            path: self.path.clone(),
            material: WorkingFile {
                file: self.material,
                leaf: self.leaf.entry().clone(),
                entity: self.entity,
            },
        }
    }
}

/// Latest instance-owned pre-history FILE result. Its original native receipt,
/// not this local frame, supplies the permanent container and working authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AddedFile {
    pub(crate) target: WorktreeFileTarget,
    pub(crate) birth: OperationId,
    pub(crate) working: UnbornWorking,
}

/// Exact independent first FILE result for an instance-owned occurrence.
/// This descriptive frame cannot construct a held source or prove publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AddedFileResult {
    pub(crate) target: WorktreeFileTarget,
    pub(crate) birth: OperationId,
    pub(crate) working: UnbornWorking,
    pub(crate) file_operation: OperationId,
    pub(crate) material: SnapshotFile,
    pub(crate) source: SnapshotFileSource,
}
impl AddedFileResult {
    pub(crate) fn require_file(&self, file: &AddedFile) -> Result<(), LedgerError> {
        if self.target != file.target || self.birth != file.birth || self.working != file.working
            || self.file_operation == self.birth
            || self.material.file_id != self.target.file
            || self.material.size != self.working.logical_size()
            || !matches!(self.source, SnapshotFileSource::Container { node, .. }
                if node == storage_contract::FileNodeId::Version(self.material.version_id))
        {
            return Err(LedgerError::FileResults);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Event {
    File { operation: OperationId, expected: Revision, file: AddedFile },
    /// Explicit completed writes merged into the retained overlay. Omission
    /// never drops an earlier edit and an unfinished upload is not an event.
    Edits { operation: OperationId, expected: Revision, edits: Vec<EditRecord> },
    /// Ordered atomic namespace intent. It never authorizes FILE publication,
    /// physical body deletion, or a production SMB namespace mutation.
    Namespace {
        operation: OperationId,
        expected: Revision,
        changes: Vec<NamespaceChange>,
    },
    Commit {
        operation: OperationId,
        expected: Revision,
        current_head: SnapshotNodeId,
        fork_branch: SnapshotBranchId,
    },
    CommitAdded {
        operation: OperationId,
        expected: Revision,
        current_head: SnapshotNodeId,
        fork_branch: SnapshotBranchId,
    },
    AddedCompleted { publication: Publication, file: AddedFileResult },
    Delete { operation: OperationId, expected: Revision },
    Completed { publication: Publication, files: Vec<FileResult> },
    /// The repository namespace owner has reserved this free mount under its fence.
    Restored { operation: OperationId, mount: Mount },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Checkpoint {
    pub(crate) sequence: Sequence,
    pub(crate) commitment: MaterialCommitment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum Format {
    WorktreeTransitions,
}
impl<'de> Deserialize<'de> for Format {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "worktree_transitions" => Ok(Self::WorktreeTransitions),
            _ => Err(data_serialization::de::Error::custom("unsupported worktree ledger format")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Genesis {
    pub(super) binding: SparseWorktreeBinding,
    pub(super) source: SnapshotSource,
    pub(super) mount: Mount,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "record", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Body {
    Genesis(Genesis),
    Event(Event),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Frame {
    pub(super) format: Format,
    pub(super) sequence: Sequence,
    pub(super) previous: Option<MaterialCommitment>,
    pub(super) body: Body,
}
impl Frame {
    pub(super) fn bytes(&self) -> Result<Vec<u8>, LedgerError> {
        let bytes = storage_contract::canonical_json(self).map_err(LedgerError::Codec)?;
        if bytes.len() > super::MAX_FRAME_BYTES {
            return Err(LedgerError::Budget);
        }
        Ok(bytes)
    }
    pub(super) fn checkpoint(&self, bytes: &[u8]) -> Checkpoint {
        Checkpoint { sequence: self.sequence, commitment: commitment(bytes) }
    }
}

pub(super) fn commitment(bytes: &[u8]) -> MaterialCommitment {
    let mut hasher = blake3::Hasher::new_derive_key("ZXS independent worktree transition frame");
    hasher.update(bytes);
    MaterialCommitment::from_digest(hasher.finalize())
}
