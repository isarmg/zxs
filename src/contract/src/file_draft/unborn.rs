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
    mode: CommitMode,
    body: UnbornBody,
}

impl UnbornWorking {
    pub const fn birth() -> Self {
        Self {
            revision: WorkingRevision::new(Generation::new(1)),
            mode: CommitMode::Manual,
            body: UnbornBody::Empty {},
        }
    }

    pub const fn revision(&self) -> WorkingRevision { self.revision }
    pub const fn mode(&self) -> CommitMode { self.mode }
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
            mode: self.mode,
            body: UnbornBody::Saved { manifest },
        })
    }

    pub fn change_mode(&self, mode: CommitMode) -> Result<Self, GenerationExhausted> {
        if self.mode == mode { return Ok(self.clone()); }
        Ok(Self { revision: self.revision.next()?, mode, body: self.body.clone() })
    }

    /// Logical result only. The canonical explicit-commit kernel consumes this
    /// after an independent full Version and its first branch actually exist.
    pub fn after_first_commit(&self) -> Result<FileWorkingState, GenerationExhausted> {
        let revision = self.revision.next()?;
        Ok(match self.mode {
            CommitMode::Manual => FileWorkingState::ManualClean { revision },
            CommitMode::Automatic => FileWorkingState::Automatic { revision },
        })
    }
}
