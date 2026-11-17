//! Private directory-side input to one selected working-file publication.
//!
//! The complete working-set picker owns working revision/manifest CAS and its
//! readers. This layer only fixes the published directory parent, exact selected
//! occurrences, and same Engine source. It cannot publish or clear any draft.
mod scope;
pub(crate) use scope::DirectoryDraftScope;
mod view;
pub(crate) use view::{DirectoryDraftFile, DirectoryDraftReader};
mod preparation;
pub(crate) use preparation::{DirectoryNodeInput, ProposedDirectoryBodies};
mod decision;
pub(crate) use decision::{
    BoundDirectoryRequest, ProposedDirectoryProjection, ReconstructionError,
};
mod batch;
pub(crate) use batch::{DirectoryWorkingError, DirectoryWorkingPlan};
mod physical;
mod publication;
mod reconstruction;
pub(in crate::snapshot_worktree) use reconstruction::reconstruct_indexed_manifest;
mod original;
pub(crate) use original::{
    ObservedOriginalDirectory, VerifiedDirectoryDraft, observe_original, verify_original,
    observe_replay_original,
};
pub(crate) use publication::{
    DirectoryDecisionInput, DirectoryPublicationError, DirectoryPublicationInput,
    PreparedDirectoryProposal,
};
pub(crate) use physical::{
    DirectoryBodyCost, DirectoryPhysicalError, DirectoryPhysicalInput, PreparedDirectoryMaterial,
};

use crate::engine::file_draft::SelectedWorkingSources;
use crate::working_tree::snapshot::HeldSnapshotNode;
use crate::Engine;
use std::sync::Arc;
use storage_contract::file_draft::WorkingSelection;
use storage_contract::snapshot_worktree::SnapshotHandle;
use storage_contract::{FileId, OperationId, Principal};

/// Non-durable input. The eventual native request codec must bind this entire
/// request before PREPARE; this value is not an operation claim or a receipt.
pub(crate) struct DirectoryDraftIntent<'a> {
    pub(crate) source: SnapshotHandle,
    pub(crate) selected: &'a [WorkingSelection],
    pub(crate) operation: OperationId,
    pub(crate) principal: &'a Principal,
    pub(crate) message: &'a str,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum DirectoryDraftError {
    #[error("reading the actual published directory source")]
    Registration(#[source] crate::snapshot_worktree::RegistrationError),
    #[error("the directory draft set requires the registered Initial root")]
    Scope,
    #[error("the directory draft selection is empty, repeated, or over budget")]
    Selection,
    #[error("the selected file is not an occurrence owned by this directory: {0}")]
    Occurrence(FileId),
    #[error("the selected path would replace another original snapshot occurrence: {0}")]
    Original(FileId),
    #[error("the directory draft plan no longer has the same admitted Engine source")]
    SourceChanged,
    #[error("directory draft preparation requires native Existing handling first: {0}")]
    Existing(OperationId),
    #[error("checking the original namespace and held owner")]
    Engine(#[source] crate::Error),
    #[error("decoding the admitted owner identity")]
    Identity(#[source] storage_contract::Error),
    #[error("reading an unchanged file from the permanently published directory node")]
    OriginalRead(#[source] crate::working_tree::snapshot::SnapshotStoreError),
    #[error("reading the bounded original directory body")]
    Read(#[source] std::io::Error),
    #[error("the proposed directory material differs from its pinned selected-body view")]
    NodeProjection,
    #[error("the complete directory body view exceeds the existing snapshot budget")]
    NodeBudget,
}

type Result<T> = std::result::Result<T, DirectoryDraftError>;

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_worktree/draft_gateway.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_worktree/draft_gateway/codec.rs"]
mod codec_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_worktree/draft_gateway/native.rs"]
mod native_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_worktree/draft_gateway/cold.rs"]
mod cold_tests;

/// A directory input paired with the unique Engine-minted complete working
/// read set. Its construction has no mutable State, Prepared payload or receipt.
pub(crate) struct PendingDirectoryDrafts {
    directory: DirectoryDraftScope,
    working: Arc<SelectedWorkingSources>,
    original: HeldSnapshotNode,
}

impl PendingDirectoryDrafts {
    pub(crate) fn capture(engine: &Engine, intent: DirectoryDraftIntent<'_>) -> Result<Arc<Self>> {
        let directory = DirectoryDraftScope::capture(engine, intent)?;
        // The single picker validates every expectation before opening any
        // selected draft. It alone distinguishes Born, Empty and Saved Unborn.
        let working = Arc::new(engine.select_working_sources(directory.selections())
            .map_err(DirectoryDraftError::Engine)?);
        let original = directory.open_original(engine)?;
        let pending = Self { directory, working, original };
        pending.require_engine(engine)?;
        Ok(Arc::new(pending))
    }

    pub(crate) fn require_engine(&self, engine: &Engine) -> Result<()> {
        self.directory.require_engine(engine)?;
        self.working.require_engine(engine).map_err(DirectoryDraftError::Engine)
    }

    pub(crate) fn require_context(
        &self,
        context: &crate::engine::PlanningContext<'_>,
    ) -> Result<()> {
        self.directory.require_context(context)?;
        self.working.require_context(context).map_err(DirectoryDraftError::Engine)
    }

    pub(crate) fn directory(&self) -> &DirectoryDraftScope {
        &self.directory
    }

    pub(crate) fn selected(&self) -> &Arc<SelectedWorkingSources> {
        &self.working
    }

    pub(crate) fn original(&self) -> &HeldSnapshotNode {
        &self.original
    }

    /// The complete new BODY view: unselected occurrences come only from the
    /// old immutable node, selected ones only from the one admitted working set.
    /// No selected file is assigned a historical VersionId by this read view.
    pub(crate) fn files(&self) -> impl Iterator<Item = DirectoryDraftFile<'_>> {
        self.directory.projection().iter().filter_map(|entry| match *entry {
            scope::DirectoryOccurrence::Original(index) => {
                let entry = &self.original.manifest().entries()[index];
                match entry.content {
                    crate::working_tree::snapshot::SnapshotEntryContent::Directory {} => None,
                    crate::working_tree::snapshot::SnapshotEntryContent::File { material, .. } => {
                        Some(DirectoryDraftFile::original(&entry.path, &self.original, material))
                    }
                }
            }
            scope::DirectoryOccurrence::Selected(index) => {
                Some(DirectoryDraftFile::selected(&self.working.sources()[index]))
            }
        })
    }
}

pub(crate) use crate::persist::directory_draft_commit::native::DirectoryEnvelopeBudget;

/// Original source proves the entire request; only the installed visible
/// outcome may be returned. This does not admit a fresh publication.
pub(crate) fn retry(
    engine: &Engine, intent: &DirectoryDraftIntent<'_>,
) -> Result<Option<crate::OperationResult>> {
    if !engine.state.operations.contains_key(&intent.operation) { return Ok(None); }
    let index = engine.snapshot_receipts().map_err(DirectoryDraftError::Registration)?;
    index.retry_working(intent).map(|result| result.cloned())
        .map_err(DirectoryDraftError::Registration)
}
