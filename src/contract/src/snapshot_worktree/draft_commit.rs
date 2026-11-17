//! Public descriptions for an explicit mixed working selection. Decoding these
//! values never admits a working source or a native receipt.
use crate::file_draft::{
    DraftBase, DraftLocation, DraftPayload, WorkingContentExpectation,
    WorkingExpectation, WorkingRevision, WorkingSelection,
};
use crate::file_draft::manifest::{ManifestCommitment, SaveGeneration};
use crate::snapshot_worktree::SnapshotHandle;
use crate::{CommitState, OperationId, ProtectionAtPublish, Sequence};
use data_serialization::{Deserialize, Serialize};

/// Shared classification only. The Engine's original-receipt admission remains
/// the sole native decoder; HTTP must never expose its physical envelope.
pub const KIND: &str = "snapshot.working.commit";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SelectedDirectoryDraft {
    pub target: DraftLocation,
    pub expected: DirectoryDraftExpectation,
}

/// Born keeps its original wire shape; first commits carry their real working
/// expectation and never a fabricated historical base.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(untagged, deny_unknown_fields)]
pub enum DirectoryDraftExpectation {
    Born { revision: WorkingRevision, base: DraftBase, payload: DraftPayload },
    Unborn { revision: WorkingRevision, content: DirectoryUnbornExpectation },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectoryUnbornExpectation {
    Empty {},
    Saved { generation: SaveGeneration, commitment: ManifestCommitment },
}

impl SelectedDirectoryDraft {
    pub fn selection(&self) -> WorkingSelection {
        let (revision, content) = match self.expected {
            DirectoryDraftExpectation::Born { revision, base, payload } => {
                (revision, WorkingContentExpectation::Born { base, payload })
            }
            DirectoryDraftExpectation::Unborn { revision, content } => {
                let content = match content {
                    DirectoryUnbornExpectation::Empty {} => WorkingContentExpectation::UnbornEmpty,
                    DirectoryUnbornExpectation::Saved { generation, commitment } => {
                        WorkingContentExpectation::UnbornSaved { generation, commitment }
                    }
                };
                (revision, content)
            }
        };
        WorkingSelection {
            target: self.target.clone(), expected: WorkingExpectation { revision, content },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CommitDirectoryDrafts {
    pub source: SnapshotHandle,
    pub selected: Vec<SelectedDirectoryDraft>,
    pub message: String,
    pub operation_id: OperationId,
}

/// Both the original scope and operation are required for a read. An operation
/// identifier is not authority to inspect another registered directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryDraftLookup {
    pub source: SnapshotHandle,
    pub operation_id: OperationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectoryDraftOutcome {
    Changed { snapshot: SnapshotHandle },
    Unchanged { snapshot: SnapshotHandle },
}

/// Only public identities and the actual visible publication state are sent.
/// The request digest remains available in the generic public receipt shell.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryDraftReceipt {
    pub operation_id: OperationId,
    pub publication_sequence: Sequence,
    pub commit_state: CommitState,
    pub protection_at_publish: ProtectionAtPublish,
    pub result: DirectoryDraftOutcome,
}

#[cfg(test)]
#[path = "../../../../test/contract/unit/directory_draft_commit.rs"]
mod tests;
