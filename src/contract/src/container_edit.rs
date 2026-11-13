//! A shared container draft is not a version, branch or private workspace.
use crate::{
    BranchId, ContentHash, EntryId, FileNodeId, Generation, GenerationExhausted, OperationId, Principal,
    RepositoryId, VersionId,
};
use data_serialization::{Deserialize, Serialize};

#[cfg(test)]
#[path = "../../../test/contract/unit/container_edit.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum CommitMode {
    Automatic,
    Manual,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SetCommitMode {
    pub file: crate::FileId,
    pub expected_generation: Generation,
    pub mode: CommitMode,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditCommand {
    SetMode { request: SetCommitMode },
    Save { file: crate::FileId, revision: DraftRevision },
    Discard { file: crate::FileId, expected_generation: Generation, draft: OperationId },
}

impl EditCommand {
    pub fn file(&self) -> crate::FileId {
        match self {
            Self::SetMode { request } => request.file,
            Self::Save { file, .. } | Self::Discard { file, .. } => *file,
        }
    }
}

/// The selected immutable source remains fixed while the shared draft changes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DraftBase {
    pub entry: EntryId,
    pub repository: RepositoryId,
    pub branch: BranchId,
    pub node: FileNodeId,
    pub version: VersionId,
    pub selection_generation: Generation,
}

/// A publication references an immutable draft body by its save operation.
/// This record grants no access without the current container authorization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DraftRevision {
    pub operation: OperationId,
    pub expected_generation: Generation,
    pub principal: Principal,
    pub base: DraftBase,
    pub content_hash: ContentHash,
    #[serialization(with = "crate::u64_string")]
    pub size: u64,
}

/// Automatic mode cannot retain an uncommitted shared body. Switching out of a
/// dirty manual state requires a separate, explicit commit or discard operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContainerEdit {
    Automatic { generation: Generation },
    ManualClean { generation: Generation },
    ManualDirty { generation: Generation, draft: DraftRevision },
}

#[derive(Clone, Debug, Eq, PartialEq, error_definition::Error)]
pub enum EditConflict {
    #[error("container edit generation changed")]
    GenerationChanged,
    #[error("commit or explicitly discard the container draft before changing selection or mode")]
    UncommittedDraft,
    #[error("automatic containers do not accept shared draft saves")]
    Automatic,
    #[error("a draft save cannot change the original selected source")]
    SourceChanged,
    #[error("container edit generation is exhausted")]
    Exhausted(#[source] GenerationExhausted),
}

impl EditConflict {
    pub fn code(&self) -> crate::ErrorCode {
        match self {
            Self::GenerationChanged | Self::SourceChanged => crate::ErrorCode::WorkspaceChanged,
            Self::UncommittedDraft | Self::Automatic => crate::ErrorCode::UnsupportedOperation,
            Self::Exhausted(_) => crate::ErrorCode::ResourceLimit,
        }
    }
}

impl ContainerEdit {
    pub const fn initial() -> Self {
        Self::Automatic { generation: Generation::new(0) }
    }

    pub fn generation(&self) -> Generation {
        match self {
            Self::Automatic { generation }
            | Self::ManualClean { generation }
            | Self::ManualDirty { generation, .. } => *generation,
        }
    }

    pub fn mode(&self) -> CommitMode {
        match self {
            Self::Automatic { .. } => CommitMode::Automatic,
            Self::ManualClean { .. } | Self::ManualDirty { .. } => CommitMode::Manual,
        }
    }

    pub fn draft(&self) -> Option<&DraftRevision> {
        match self {
            Self::Automatic { .. } | Self::ManualClean { .. } => None,
            Self::ManualDirty { draft, .. } => Some(draft),
        }
    }

    pub fn require_clean(&self) -> Result<(), EditConflict> {
        match self {
            Self::ManualDirty { .. } => Err(EditConflict::UncommittedDraft),
            Self::Automatic { .. } | Self::ManualClean { .. } => Ok(()),
        }
    }

    pub fn set_mode(
        &self,
        expected: Generation,
        mode: CommitMode,
    ) -> Result<Self, EditConflict> {
        self.require_generation(expected)?;
        if self.mode() == mode {
            return Ok(self.clone());
        }
        self.require_clean()?;
        let generation = self.generation().next().map_err(EditConflict::Exhausted)?;
        Ok(match mode {
            CommitMode::Automatic => Self::Automatic { generation },
            CommitMode::Manual => Self::ManualClean { generation },
        })
    }

    pub fn save(
        &self,
        expected: Generation,
        revision: DraftRevision,
    ) -> Result<Self, EditConflict> {
        self.require_generation(expected)?;
        if revision.expected_generation != expected {
            return Err(EditConflict::GenerationChanged);
        }
        match self {
            Self::Automatic { .. } => return Err(EditConflict::Automatic),
            Self::ManualClean { .. } => {}
            Self::ManualDirty { draft, .. } => {
                if draft.base != revision.base {
                    return Err(EditConflict::SourceChanged);
                }
            }
        }
        Ok(Self::ManualDirty {
            generation: self.generation().next().map_err(EditConflict::Exhausted)?,
            draft: revision,
        })
    }

    pub fn discard(
        &self,
        expected: Generation,
        operation: OperationId,
    ) -> Result<Self, EditConflict> {
        self.require_generation(expected)?;
        match self {
            Self::ManualDirty { draft, .. } if draft.operation == operation => {
                Ok(Self::ManualClean {
                    generation: self.generation().next().map_err(EditConflict::Exhausted)?,
                })
            }
            Self::Automatic { .. } => Err(EditConflict::Automatic),
            Self::ManualClean { .. } | Self::ManualDirty { .. } => {
                Err(EditConflict::GenerationChanged)
            }
        }
    }

    fn require_generation(&self, expected: Generation) -> Result<(), EditConflict> {
        if self.generation() != expected {
            return Err(EditConflict::GenerationChanged);
        }
        Ok(())
    }
}
