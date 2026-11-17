//! Explicit first-history request. Empty means no historical base, not Version zero.
use data_serialization::{Deserialize, Serialize};
use storage_contract::OperationId;
use storage_contract::file_draft::{
    CommitMode, DraftLocation, UnbornBody, UnbornWorking, WorkingContentExpectation,
    WorkingExpectation, WorkingRevision, WorkingSelection,
};
use storage_contract::file_draft::manifest::{ManifestCommitment, SaveGeneration};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnbornContent {
    Empty {},
    Saved { generation: SaveGeneration, commitment: ManifestCommitment },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct UnbornExpectation {
    pub revision: WorkingRevision,
    pub content: UnbornContent,
}

impl UnbornExpectation {
    pub(crate) fn from_working(working: &UnbornWorking) -> Self {
        let content = match working.body() {
            UnbornBody::Empty {} => UnbornContent::Empty {},
            UnbornBody::Saved { manifest } => UnbornContent::Saved {
                generation: manifest.generation(), commitment: manifest.commitment(),
            },
        };
        Self { revision: working.revision(), content }
    }

    pub(crate) fn selection(&self, target: DraftLocation) -> WorkingSelection {
        let content = match self.content {
            UnbornContent::Empty {} => WorkingContentExpectation::UnbornEmpty,
            UnbornContent::Saved { generation, commitment } => {
                WorkingContentExpectation::UnbornSaved { generation, commitment }
            }
        };
        WorkingSelection {
            target, expected: WorkingExpectation { revision: self.revision, content },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct CommitUnbornFile {
    pub operation_id: OperationId,
    pub target: DraftLocation,
    pub expected: UnbornExpectation,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct UnbornFileSnapshot {
    pub target: DraftLocation,
    pub expected: UnbornExpectation,
    pub mode: CommitMode,
    #[serialization(with = "storage_contract::u64_string")]
    pub logical_size: u64,
}
