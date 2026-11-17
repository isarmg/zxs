//! An explicit commit's expected working occurrence, not a storage capability.
//! Transport receipts have their own codec; these values cannot be deserialized
//! into an admitted reader, a save receipt or a historical publication.
use super::manifest::{ManifestCommitment, SaveGeneration};
use super::{DraftBase, DraftLocation, DraftPayload, FileWorkingState, UnbornBody, WorkingRevision};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkingContentExpectation {
    Born { base: DraftBase, payload: DraftPayload },
    UnbornEmpty,
    UnbornSaved { generation: SaveGeneration, commitment: ManifestCommitment },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkingExpectation {
    pub revision: WorkingRevision,
    pub content: WorkingContentExpectation,
}

impl WorkingExpectation {
    /// Descriptive request material only. The live picker compares the complete
    /// value with the same admitted State before opening any selected body.
    pub fn from_working(working: &FileWorkingState) -> Option<Self> {
        let content = match working {
            FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. } => {
                return None;
            }
            FileWorkingState::ManualDirty { base, payload, .. }
            | FileWorkingState::ManualIncremental { base, payload, .. } => {
                WorkingContentExpectation::Born { base: *base, payload: *payload }
            }
            FileWorkingState::Unborn { working } => match working.body() {
                UnbornBody::Empty {} => WorkingContentExpectation::UnbornEmpty,
                UnbornBody::Saved { manifest } => WorkingContentExpectation::UnbornSaved {
                    generation: manifest.generation(),
                    commitment: manifest.commitment(),
                },
            },
        };
        Some(Self { revision: working.revision(), content })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingSelection {
    pub target: DraftLocation,
    pub expected: WorkingExpectation,
}
