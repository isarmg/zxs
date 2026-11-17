//! Shared draft transitions and their exact occurrence/publication consumers.
//!
//! The logical mode/revision core issues no storage authority. The content
//! adapter consumes the retained draft source and the real Version constructor;
//! publication replay closes the complete State before any physical grant.
use storage_contract::GenerationExhausted;
mod allocations;
mod content;
pub(crate) mod batch;
pub(crate) use content::{PublishedContent, build as build_content, trace_limit};
mod selection;
pub(crate) mod selected;
pub(crate) mod unborn;
pub(crate) mod worktree;
pub(crate) use selection::{resolve, validate as validate_working_record};
mod publication;
pub(crate) use publication::{
    Outcome, apply, draft_content_hash, draft_matches_base, plan_change, plan_incremental, verify_working_transition,
};
use storage_contract::file_draft::{
    CommitMode, DraftBase, DraftPayload, FileWorkingState, WorkingRevision,
};

#[derive(Debug, error_definition::Error)]
pub enum DraftError {
    #[error("the exact worktree FILE publication could not complete")]
    Worktree(#[source] Box<crate::SnapshotWorktreeError>),
    #[error("this operation requires the canonical unborn-container transition")]
    UnbornRequired,
    #[error("unborn working material is invalid")]
    WorkingManifest(#[source] storage_contract::file_draft::manifest::ManifestError),
    #[error("the current namespace has conflicting or unbound container identities")]
    WorkingNamespace,
    #[error("unborn working material changed its named entity or immutable bytes")]
    WorkingMaterialChanged,
    #[error("the private working edit batch failed and cannot be published")]
    WorkingEditFailed,
    #[error("the working edit does not name its current visible length")]
    WorkingLength,
    #[error("working material block count exceeds this platform's bounded domain")]
    WorkingCount(#[source] std::num::TryFromIntError),
    #[error("the admitted owner cannot reserve working extents and its metadata tail")]
    WorkingCapacity,
    #[error("unborn FILE material must contain only its one current container row")]
    WorkingFileGraph,
    #[error("the explicit working selection is empty, repeated or over budget")]
    WorkingSelection,
    #[error("the selected working body differs from the exact expected save")]
    WorkingBodyChanged,
    #[error("manual draft is outside its exact selected root-level FILE occurrence")]
    Selection,
    #[error("this manual draft operation requires an Initial single-owner container")]
    UnsupportedScope,
    #[error("the manual draft publication no longer has its original source")]
    SourceChanged,
    #[error("the permanent manual draft publication differs from its exact transition")]
    Publication,
    #[error("encoding or decoding the permanent manual draft receipt")]
    Encoding(#[source] json_codec::Error),
    #[error("the manual draft exceeds the configured complete-file budget")]
    Size,
    #[error("the explicit commit body differs from its published draft")]
    Body,
    #[error("the manual draft publication generation is exhausted")]
    PublicationGeneration(#[source] GenerationExhausted),
    #[error("working revision changed: expected {expected}, found {actual}")]
    RevisionChanged {
        expected: WorkingRevision,
        actual: WorkingRevision,
    },
    #[error("manual commit mode is required for a shared draft write")]
    ManualRequired,
    #[error("automatic mode ended; the old automatic handle cannot save a history node")]
    AutomaticRequired,
    #[error("the shared draft has no uncommitted body")]
    DraftRequired,
    #[error("commit or discard the shared draft before changing mode or selection")]
    Dirty,
    #[error("the write does not identify the shared draft's original selection")]
    BaseChanged,
    #[error("the shared working revision is exhausted")]
    RevisionExhausted(#[source] GenerationExhausted),
}

impl DraftError {
    pub(crate) fn code(&self) -> storage_contract::ErrorCode {
        use storage_contract::ErrorCode;
        match self {
            Self::Worktree(source) => source.code(),
            Self::UnbornRequired => ErrorCode::UnsupportedOperation,
            Self::WorkingManifest(_) | Self::WorkingNamespace => ErrorCode::StateInvalid,
            Self::WorkingMaterialChanged => ErrorCode::ContainerLayoutInvalid,
            Self::WorkingEditFailed => ErrorCode::RecoveryRequired,
            Self::WorkingLength => ErrorCode::FileVersionConflict,
            Self::WorkingCount(_) => ErrorCode::ResourceLimit,
            Self::WorkingCapacity => ErrorCode::NoSpace,
            Self::WorkingFileGraph => ErrorCode::ContainerLayoutInvalid,
            Self::WorkingSelection => ErrorCode::InvalidRequest,
            Self::WorkingBodyChanged => ErrorCode::FileBranchConflict,
            Self::Selection => ErrorCode::FileBranchConflict,
            Self::UnsupportedScope => ErrorCode::UnsupportedOperation,
            Self::SourceChanged => ErrorCode::PreparationStale,
            Self::Publication | Self::Encoding(_) => ErrorCode::PublicationInvalid,
            Self::Size | Self::RevisionExhausted(_) | Self::PublicationGeneration(_) => {
                ErrorCode::ResourceLimit
            }
            Self::Body => ErrorCode::ChecksumMismatch,
            Self::RevisionChanged { .. }
            | Self::ManualRequired
            | Self::AutomaticRequired
            | Self::DraftRequired
            | Self::Dirty
            | Self::BaseChanged => ErrorCode::FileBranchConflict,
        }
    }
}

pub(crate) fn invalid() -> crate::Error {
    crate::Error::FileDraft(DraftError::Publication)
}

/// A logical change, not permission to install or remove either draft body.
pub(crate) struct WorkingChange<'a> {
    previous: &'a FileWorkingState,
    next: FileWorkingState,
}

impl WorkingChange<'_> {
    pub(crate) fn previous(&self) -> &FileWorkingState {
        self.previous
    }

    pub(crate) fn next(&self) -> &FileWorkingState {
        &self.next
    }

    pub(crate) fn changed(&self) -> bool {
        self.previous != &self.next
    }

    pub(crate) fn into_next(self) -> FileWorkingState {
        self.next
    }
}

/// The exact body and first-dirty selection consumed by an explicit commit.
/// The real publication must construct its content result before consuming this
/// change; merely planning a commit does not clear the authoritative draft.
pub(crate) struct ManualCommit<'a> {
    previous: &'a FileWorkingState,
    base: &'a DraftBase,
    payload: &'a DraftPayload,
    next_revision: WorkingRevision,
}

impl<'a> ManualCommit<'a> {
    pub(crate) fn base(&self) -> &'a DraftBase {
        self.base
    }

    pub(crate) fn payload(&self) -> &'a DraftPayload {
        self.payload
    }

    pub(crate) fn into_change(self) -> WorkingChange<'a> {
        WorkingChange {
            previous: self.previous,
            next: FileWorkingState::ManualClean {
                revision: self.next_revision,
            },
        }
    }
}

fn require_revision(
    current: &FileWorkingState,
    expected: WorkingRevision,
) -> Result<(), DraftError> {
    let actual = current.revision();
    if actual != expected {
        return Err(DraftError::RevisionChanged { expected, actual });
    }
    Ok(())
}

fn next_revision(current: &FileWorkingState) -> Result<WorkingRevision, DraftError> {
    current
        .revision()
        .next()
        .map_err(DraftError::RevisionExhausted)
}

pub(crate) fn change_mode(
    current: &FileWorkingState,
    expected: WorkingRevision,
    mode: CommitMode,
) -> Result<WorkingChange<'_>, DraftError> {
    require_revision(current, expected)?;
    if current.mode() == mode {
        return Ok(WorkingChange {
            previous: current,
            next: current.clone(),
        });
    }
    require_clean(current)?;
    let revision = next_revision(current)?;
    let next = match mode {
        CommitMode::Automatic => FileWorkingState::Automatic { revision },
        CommitMode::Manual => FileWorkingState::ManualClean { revision },
    };
    Ok(WorkingChange {
        previous: current,
        next,
    })
}

pub(crate) fn write(
    current: &FileWorkingState,
    expected: WorkingRevision,
    base: DraftBase,
    payload: DraftPayload,
) -> Result<WorkingChange<'_>, DraftError> {
    require_revision(current, expected)?;
    match current {
        FileWorkingState::Unborn { .. } => return Err(DraftError::UnbornRequired),
        FileWorkingState::Automatic { .. } => return Err(DraftError::ManualRequired),
        FileWorkingState::ManualClean { .. } => {}
        FileWorkingState::ManualDirty { base: original, .. }
        | FileWorkingState::ManualIncremental { base: original, .. } => {
            if *original != base {
                return Err(DraftError::BaseChanged);
            }
        }
    }
    let revision = next_revision(current)?;
    Ok(WorkingChange {
        previous: current,
        next: FileWorkingState::ManualDirty {
            revision,
            base,
            payload,
        },
    })
}

pub(crate) fn discard(
    current: &FileWorkingState,
    expected: WorkingRevision,
) -> Result<WorkingChange<'_>, DraftError> {
    require_revision(current, expected)?;
    let next = match current {
        FileWorkingState::Unborn { .. } => return Err(DraftError::UnbornRequired),
        FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. } => {
            current.clone()
        }
        FileWorkingState::ManualDirty { .. } | FileWorkingState::ManualIncremental { .. }
            => FileWorkingState::ManualClean {
            revision: next_revision(current)?,
        },
    };
    Ok(WorkingChange {
        previous: current,
        next,
    })
}

pub(crate) fn commit(
    current: &FileWorkingState,
    expected: WorkingRevision,
) -> Result<ManualCommit<'_>, DraftError> {
    require_revision(current, expected)?;
    match current {
        FileWorkingState::Unborn { .. } => Err(DraftError::UnbornRequired),
        FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. } => {
            Err(DraftError::DraftRequired)
        }
        FileWorkingState::ManualDirty { base, payload, .. }
        | FileWorkingState::ManualIncremental { base, payload, .. } => Ok(ManualCommit {
            previous: current,
            base,
            payload,
            next_revision: next_revision(current)?,
        }),
    }
}

pub(crate) fn require_clean(current: &FileWorkingState) -> Result<(), DraftError> {
    match current {
        FileWorkingState::Unborn { .. } => Err(DraftError::UnbornRequired),
        FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. } => Ok(()),
        FileWorkingState::ManualDirty { .. } | FileWorkingState::ManualIncremental { .. }
            => Err(DraftError::Dirty),
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_draft_tests.rs"]
mod tests;
