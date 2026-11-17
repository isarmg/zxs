//! Working state of the existing container owner before its first history node.
use super::manifest::{ManifestError, ManifestPrevious, WorkingManifest};
use super::{CommitMode, FileWorkingState, WorkingRevision};
use crate::{FileId, Generation, GenerationExhausted};
use data_serialization::{Deserialize, Serialize};

/// Creating a current container is distinct from committing a historical Entry.
/// The owning disk and new stable identities are derived by the engine from the
/// registered namespace after exact operation replay has been checked.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BirthRequest {
    pub repository: crate::RepositoryId,
    pub path: name_policy::PortablePath,
    pub expected_namespace_head: crate::CommitId,
}

/// One ordered edit batch, frozen only after actual immutable extent feedback.
/// This is descriptive request data; only the engine's held preparation can
/// introduce these extents into a new publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct UnbornSaveRequest {
    pub target: super::DraftLocation,
    pub expected: WorkingRevision,
    pub controller: super::SaveController,
    pub manifest: WorkingManifest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnbornBody {
    /// Container birth is a readable empty file, not a saved empty Version.
    Empty {},
    Saved { manifest: WorkingManifest },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct UnbornWorking {
    revision: WorkingRevision,
    controller: BirthController,
    last: UnbornDecision,
    body: UnbornBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnbornDecision { Birth {}, Mode { operation: crate::OperationId }, Save { operation: crate::OperationId } }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum BirthController {
    Manual {},
    Automatic { controller: super::automatic::AutomaticController },
}

impl UnbornWorking {
    pub const fn birth() -> Self {
        Self {
            revision: WorkingRevision::new(Generation::new(1)),
            controller: BirthController::Manual {},
            last: UnbornDecision::Birth {},
            body: UnbornBody::Empty {},
        }
    }

    pub const fn revision(&self) -> WorkingRevision { self.revision }
    pub const fn mode(&self) -> CommitMode {
        match &self.controller {
            BirthController::Manual {} => CommitMode::Manual,
            BirthController::Automatic { .. } => CommitMode::Automatic,
        }
    }

    pub const fn automatic_controller(&self) -> Option<&super::automatic::AutomaticController> {
        match &self.controller {
            BirthController::Manual {} => None,
            BirthController::Automatic { controller } => Some(controller),
        }
    }
    pub const fn last_decision(&self) -> UnbornDecision { self.last }
    pub const fn body(&self) -> &UnbornBody { &self.body }

    pub const fn logical_size(&self) -> u64 {
        match &self.body {
            UnbornBody::Empty {} => 0,
            UnbornBody::Saved { manifest } => manifest.logical_size(),
        }
    }

    pub fn saved(
        &self,
        file: FileId,
        manifest: WorkingManifest,
        maximum: u64,
    ) -> Result<Self, ManifestError> {
        if self.automatic_controller().is_some() { return Err(ManifestError::Predecessor); }
        self.saved_body(file, manifest, maximum)
    }

    fn saved_body(&self, file: FileId, manifest: WorkingManifest, maximum: u64)
        -> Result<Self, ManifestError>
    {
        manifest.validate(maximum)?;
        if manifest.file() != file {
            return Err(ManifestError::Predecessor);
        }
        let previous = match &self.body {
            UnbornBody::Empty {} => ManifestPrevious::Birth {},
            UnbornBody::Saved { manifest: previous } => {
                manifest.require_successor_of(previous, maximum)?;
                ManifestPrevious::Saved {
                    generation: previous.generation(),
                    commitment: previous.commitment(),
                    visible_length: previous.logical_size(),
                }
            }
        };
        if manifest.previous() != previous {
            return Err(ManifestError::Predecessor);
        }
        Ok(Self {
            revision: self.revision.next().map_err(ManifestError::Generation)?,
            controller: self.controller.clone(),
            last: UnbornDecision::Save { operation: manifest.operation() },
            body: UnbornBody::Saved { manifest },
        })
    }

    pub fn saved_decision(&self, file: FileId, manifest: WorkingManifest, maximum: u64,
        sequence: crate::Sequence, time: crate::Timestamp) -> Result<Self, UnbornSaveError>
    {
        let operation = manifest.operation();
        let mut after = self.saved_body(file, manifest, maximum).map_err(UnbornSaveError::Manifest)?;
        if let BirthController::Automatic { controller } = &self.controller {
            after.controller = BirthController::Automatic {
                controller: controller.saved(after.revision, operation, sequence, time).map_err(UnbornSaveError::Controller)?,
            };
        }
        Ok(after)
    }

    pub fn resume(&self, epoch: Generation, operation: crate::OperationId) -> Result<Self, UnbornSaveError> {
        let Some(controller) = self.automatic_controller() else {
            return Err(UnbornSaveError::Controller(super::automatic::ControllerError::Fence));
        };
        let revision = self.revision.next().map_err(|error| UnbornSaveError::Manifest(ManifestError::Generation(error)))?;
        Ok(Self { revision, last: UnbornDecision::Mode { operation }, body: self.body.clone(),
            controller: BirthController::Automatic { controller: controller.resumed(epoch, revision)
                .map_err(UnbornSaveError::Controller)? } })
    }

    pub fn change_mode(&self, mode: CommitMode, operation: crate::OperationId) -> Result<Self, GenerationExhausted> {
        if self.mode() == mode { return Ok(self.clone()); }
        let revision = self.revision.next()?;
        let controller = match mode {
            CommitMode::Manual => BirthController::Manual {},
            CommitMode::Automatic => BirthController::Automatic {
                controller: super::automatic::AutomaticController::enabled(revision,
                    super::automatic::AutomaticPolicy::TwentySavesInSixtySeconds),
            },
        };
        Ok(Self { revision, controller, last: UnbornDecision::Mode { operation }, body: self.body.clone() })
    }

    /// Logical result only. The canonical explicit-commit kernel consumes this
    /// after an independent full Version and its first branch actually exist.
    pub fn after_first_commit(&self) -> Result<FileWorkingState, GenerationExhausted> {
        let revision = self.revision.next()?;
        Ok(match &self.controller {
            BirthController::Manual {} => FileWorkingState::ManualClean { revision },
            BirthController::Automatic { controller } => FileWorkingState::Automatic {
                revision, controller: controller.consumed_manually(), body: super::AutomaticBody::Clean {},
            },
        })
    }
}

#[derive(Debug, error_definition::Error)]
pub enum UnbornSaveError {
    #[error("invalid Unborn manifest transition")]
    Manifest(#[source] ManifestError),
    #[error("invalid Unborn automatic save transition")]
    Controller(#[source] super::automatic::ControllerError),
}
