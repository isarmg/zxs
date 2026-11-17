//! A real FILE batch supplies historical identities for the pinned directory view.
//! No native decision, physical node preparation or installed working clear is issued.
use crate::engine::PlanningContext;
use crate::engine::file_draft::{PreparedWorkingBatch, WorkingBatchResult};
use crate::persist::directory_draft_commit::{
    ConstructionDescription, FileOutcome, SelectedOutcome,
};
use crate::snapshot_worktree::drafts::{
    BoundDirectoryRequest, DirectoryDraftError, DirectoryNodeInput, PendingDirectoryDrafts,
    ProposedDirectoryBodies, ProposedDirectoryProjection, ReconstructionError,
};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotFile};
use storage_contract::working_tree::SnapshotNodeId;
use std::sync::Arc;

#[derive(Debug, error_definition::Error)]
pub(crate) enum DirectoryWorkingError {
    #[error("retaining the published directory and its exact working selection")]
    Gateway(#[source] DirectoryDraftError),
    #[error("consuming the same held FILE batch and its complete reconstruction")]
    Batch(#[source] crate::Error),
    #[error("the FILE batch does not belong to this complete directory request")]
    Request,
    #[error("decoding the actual constructed FILE version identity")]
    Identity(#[source] storage_contract::Error),
    #[error("reconstructing the directory from the actual FILE batch")]
    Reconstruction(#[source] ReconstructionError),
}

type Result<T> = std::result::Result<T, DirectoryWorkingError>;

/// Owns the non-clone FILE producer and retains its original directory source.
/// The description is generated from that producer, never accepted in its place.
/// Physical capacity, node feedback and one native receipt remain later gates.
pub(crate) struct DirectoryWorkingPlan {
    pub(super) input: DirectoryNodeInput,
    pub(super) batch: PreparedWorkingBatch,
    pub(super) construction: ConstructionDescription,
    pub(super) projection: ProposedDirectoryProjection,
}

impl PendingDirectoryDrafts {
    pub(crate) fn bind_working_batch(
        self: &Arc<Self>,
        context: &PlanningContext<'_>,
        batch: PreparedWorkingBatch,
        node: SnapshotNodeId,
    ) -> Result<DirectoryWorkingPlan> {
        self.require_context(context).map_err(DirectoryWorkingError::Gateway)?;
        batch.require_context(context).map_err(DirectoryWorkingError::Batch)?;
        batch.require_selection(self.selected()).map_err(DirectoryWorkingError::Batch)?;
        let request = batch.request();
        let directory = self.directory();
        if request.operation() != directory.operation()
            || request.principal() != directory.principal()
            || request.message() != directory.message()
            || request.selected() != directory.selections()
        {
            return Err(DirectoryWorkingError::Request);
        }
        batch.require_reconstruction(context).map_err(DirectoryWorkingError::Batch)?;
        let input = self.node_input(context).map_err(DirectoryWorkingError::Gateway)?;
        let bound = BoundDirectoryRequest::capture(self, context)
            .map_err(DirectoryWorkingError::Reconstruction)?;
        let files = describe_items(batch.items())?;
        let construction = ConstructionDescription {
            request: bound.request().clone(), node, files,
        };
        let projection = bound.reconstruct_proposed(context, &construction)
            .map_err(DirectoryWorkingError::Reconstruction)?;
        input.bind_proposed_manifest(projection.manifest())
            .map_err(DirectoryWorkingError::Gateway)?;
        let result = DirectoryWorkingPlan { input, batch, construction, projection };
        result.require_context(context)?;
        Ok(result)
    }
}

impl DirectoryWorkingPlan {
    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.input.require_context(context).map_err(DirectoryWorkingError::Gateway)?;
        self.batch.require_context(context).map_err(DirectoryWorkingError::Batch)
    }

    pub(crate) fn files(&self) -> &PreparedWorkingBatch {
        &self.batch
    }

    pub(crate) fn construction(&self) -> &ConstructionDescription {
        &self.construction
    }

    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest {
        self.projection.manifest()
    }

    pub(crate) fn bodies(&self) -> Result<ProposedDirectoryBodies<'_>> {
        self.input.bind_proposed_manifest(self.manifest()).map_err(DirectoryWorkingError::Gateway)
    }
}

pub(super) fn describe_items(
    items: &[crate::business::file_draft::batch::WorkingBatchItem],
) -> Result<Vec<SelectedOutcome>> {
    items.iter().map(|item| {
            let version = item.version();
            let material = SnapshotFile {
                file_id: item.target().file,
                version_id: version.version_id.parse().map_err(DirectoryWorkingError::Identity)?,
                size: version.logical_size,
                content_hash: version.content_hash,
            };
            let outcome = match item.result() {
                WorkingBatchResult::Retained(_) => FileOutcome::Retained {
                    base: item.base(), material,
                },
                WorkingBatchResult::Published(_) => FileOutcome::Published {
                    base: item.base(), branch: item.branch(),
                    node: item.node().map_err(DirectoryWorkingError::Batch)?, material,
                },
            };
            Ok(SelectedOutcome { target: item.target().clone(), outcome })
    }).collect()
}
