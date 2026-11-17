//! One owned preparation, one physical feedback, and one prospective receipt.
//! The internal consumer still requires authenticated new-pool admission.
use super::{
    DirectoryDraftError, DirectoryPhysicalError, DirectoryPhysicalInput,
    DirectoryWorkingError, DirectoryWorkingPlan, PendingDirectoryDrafts,
    PreparedDirectoryMaterial,
};
use crate::engine::file_draft::{PreparedWorkingBatch, WorkingBatchHandoff};
use crate::engine::PlanningContext;
use crate::persist::directory_draft_commit::native::{
    DirectoryEnvelopeBudget, Envelope, NativeError, NativePlan,
};
use crate::{ErrorCode, OperationResult, State};
use std::sync::Arc;
use storage_contract::Sequence;

#[derive(Debug, error_definition::Error)]
pub(crate) enum DirectoryPublicationError {
    #[error("indexing the original directory receipts in the verified visible prefix")]
    ReceiptIndex(#[source] crate::snapshot_worktree::SnapshotWorktreeError),
    #[error("retaining the exact original directory and working sources")]
    Source(#[source] DirectoryDraftError),
    #[error("consuming the complete selected FILE construction")]
    Working(#[source] DirectoryWorkingError),
    #[error("checking the actual FILE construction and publication gates")]
    Engine(#[source] crate::Error),
    #[error("preparing the original complete directory material")]
    Physical(#[source] DirectoryPhysicalError),
    #[error("encoding or admitting the complete combined decision")]
    Native(#[source] NativeError),
    #[error("the actual combined decision differs from its complete reconstruction")]
    Projection,
    #[error("the actual Candidate differs from this original combined decision")]
    Candidate,
}

impl DirectoryPublicationError {
    pub(crate) fn into_engine(self) -> crate::Error {
        crate::snapshot_worktree::source::directory_publication(self)
    }

    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::ReceiptIndex(source) => source.code(),
            Self::Engine(source) => source.code(),
            Self::Physical(DirectoryPhysicalError::Admission(source)) => source.code(),
            Self::Physical(DirectoryPhysicalError::Budget) => ErrorCode::ResourceLimit,
            Self::Physical(_) => ErrorCode::ContainerLayoutInvalid,
            Self::Source(source) => source_code(source),
            Self::Working(DirectoryWorkingError::Gateway(source)) => source_code(source),
            Self::Working(DirectoryWorkingError::Batch(source)) => source.code(),
            Self::Working(_) => ErrorCode::StateInvalid,
            Self::Native(source) => source.code(),
            Self::Projection | Self::Candidate => ErrorCode::PublicationInvalid,
        }
    }
}

fn source_code(source: &DirectoryDraftError) -> ErrorCode {
    match source {
        DirectoryDraftError::Registration(source) => {
            crate::snapshot_worktree::SnapshotWorktreeError::code_for_registration(source)
        }
        DirectoryDraftError::Engine(source) => source.code(),
        DirectoryDraftError::Scope => ErrorCode::UnsupportedOperation,
        DirectoryDraftError::Selection | DirectoryDraftError::Occurrence(_)
        | DirectoryDraftError::Original(_) => ErrorCode::InvalidRequest,
        DirectoryDraftError::SourceChanged => ErrorCode::PreparationStale,
        DirectoryDraftError::Existing(_) => ErrorCode::IdempotencyKeyReused,
        DirectoryDraftError::NodeBudget => ErrorCode::ResourceLimit,
        DirectoryDraftError::Identity(_) | DirectoryDraftError::OriginalRead(_)
        | DirectoryDraftError::Read(_) | DirectoryDraftError::NodeProjection => {
            ErrorCode::ContainerLayoutInvalid
        }
    }
}

type Result<T> = std::result::Result<T, DirectoryPublicationError>;

pub(crate) struct DirectoryPublicationInput {
    decision: DirectoryDecisionInput,
    files: PreparedWorkingBatch,
    budget: DirectoryEnvelopeBudget,
}

/// Non-Clone. The only constructor consumes the actual plan and FILE batch.
/// Its descriptive envelope does not by itself allow node creation or clearing.
pub(crate) struct DirectoryDecisionInput {
    source: Arc<PendingDirectoryDrafts>,
    physical: Arc<DirectoryPhysicalInput>,
    native: NativePlan,
    maximum: usize,
}

impl DirectoryWorkingPlan {
    pub(crate) fn into_publication(
        self, context: &PlanningContext<'_>, maximum: usize,
    ) -> Result<DirectoryPublicationInput> {
        context.require_manual_directory_pool().map_err(DirectoryPublicationError::Engine)?;
        self.require_context(context).map_err(DirectoryPublicationError::Working)?;
        let Self { input, batch, construction, projection } = self;
        let source = Arc::clone(input.source());
        batch.require_reconstruction(context).map_err(DirectoryPublicationError::Engine)?;
        let physical = Arc::new(DirectoryPhysicalInput::new(
            input, projection.into_manifest(), context,
        ).map_err(DirectoryPublicationError::Physical)?);
        let native = NativePlan::from_batch(
            context.state, construction, &batch,
            crate::business::now().map_err(DirectoryPublicationError::Engine)?,
            context.config.membership_epoch(), context.config.max_entries,
        ).map_err(DirectoryPublicationError::Native)?;
        let budget = native.budget(&physical, maximum).map_err(DirectoryPublicationError::Native)?;
        let decision = DirectoryDecisionInput { source, physical, native, maximum };
        batch.require_selection(decision.source.selected()).map_err(DirectoryPublicationError::Engine)?;
        Ok(DirectoryPublicationInput { decision, files: batch, budget })
    }
}

impl DirectoryPublicationInput {
    /// The real publication pipeline receives each source only once. The
    /// independent physical input and decision retain the identical Arc.
    pub(crate) fn into_material(
        self, context: &PlanningContext<'_>,
    ) -> Result<(
        DirectoryDecisionInput, PreparedWorkingBatch,
        Arc<DirectoryPhysicalInput>, DirectoryEnvelopeBudget,
    )> {
        self.files.require_context(context).map_err(DirectoryPublicationError::Engine)?;
        self.files.require_selection(self.decision.source.selected()).map_err(DirectoryPublicationError::Engine)?;
        self.decision.source.require_context(context).map_err(DirectoryPublicationError::Source)?;
        self.decision.physical.require_context(context).map_err(DirectoryPublicationError::Physical)?;
        let physical = Arc::clone(&self.decision.physical);
        Ok((self.decision, self.files, physical, self.budget))
    }
}

impl DirectoryDecisionInput {
    pub(crate) fn require_context(
        &self, context: &PlanningContext<'_>, files: &WorkingBatchHandoff,
    ) -> Result<()> {
        self.source.require_context(context).map_err(DirectoryPublicationError::Source)?;
        files.require_context(context).map_err(DirectoryPublicationError::Engine)?;
        files.require_selection(self.source.selected()).map_err(DirectoryPublicationError::Engine)?;
        self.physical.require_context(context).map_err(DirectoryPublicationError::Physical)
    }

    /// Only actual EXCL/readback feedback can close the prospective receipt.
    /// The caller must still perform the final exact metadata budget and bind
    /// the original snapshot/Candidate before it can decide anything.
    pub(crate) fn finish(
        self, context: &PlanningContext<'_>, files: &WorkingBatchHandoff,
        material: &PreparedDirectoryMaterial,
    ) -> Result<PreparedDirectoryProposal> {
        self.require_context(context, files)?;
        material.require_input(&self.physical).map_err(DirectoryPublicationError::Physical)?;
        material.require_context(context).map_err(DirectoryPublicationError::Physical)?;
        let envelope = self.native.feedback(&self.physical, material.binding())
            .map_err(DirectoryPublicationError::Native)?;
        let result = envelope.receipt(
            context.state, Sequence::new(context.next_slot),
            self.maximum, context.config.max_entries,
        ).map_err(DirectoryPublicationError::Native)?;
        let index = context.snapshot_receipts()
            .map_err(|source| DirectoryPublicationError::ReceiptIndex(source.into()))?;
        let replay = super::reconstruction::replay(
            context.config, context.state, &index, &result, self.maximum, &files.first_reads()
                .map_err(DirectoryPublicationError::Engine)?, files.draft_reads(),
        )?;
        replay.require_file_projection(context.state, files.state())?;
        let next = replay.into_state();
        self.require_context(context, files)?;
        material.require_context(context).map_err(DirectoryPublicationError::Physical)?;
        Ok(PreparedDirectoryProposal { next, result, envelope, source: self })
    }
}

/// This is a candidate proposal, not a Published receipt or installed State.
/// Its source guard must remain alive through the original publication phases.
pub(crate) struct PreparedDirectoryProposal {
    next: State,
    result: OperationResult,
    envelope: Envelope,
    source: DirectoryDecisionInput,
}

impl PreparedDirectoryProposal {
    pub(crate) fn state(&self) -> &State { &self.next }
    pub(crate) fn result(&self) -> &OperationResult { &self.result }

    pub(crate) fn require_context(
        &self, context: &PlanningContext<'_>, files: &WorkingBatchHandoff,
        material: &PreparedDirectoryMaterial,
    ) -> Result<()> {
        self.source.require_context(context, files)?;
        material.require_input(&self.source.physical).map_err(DirectoryPublicationError::Physical)?;
        material.require_context(context).map_err(DirectoryPublicationError::Physical)
    }

    pub(crate) fn require_material(
        &self, files: &WorkingBatchHandoff, material: &PreparedDirectoryMaterial,
    ) -> Result<()> {
        files.require_selection(self.source.source.selected())
            .map_err(DirectoryPublicationError::Engine)?;
        files.require_material().map_err(DirectoryPublicationError::Engine)?;
        material.require_input(&self.source.physical).map_err(DirectoryPublicationError::Physical)
    }

    pub(crate) fn verified<'a>(
        &'a self, context: &PlanningContext<'a>, files: &WorkingBatchHandoff,
        material: &PreparedDirectoryMaterial,
    ) -> Result<super::original::VerifiedDirectoryDraft<'a>> {
        self.require_context(context, files, material)?;
        super::original::VerifiedDirectoryDraft::from_live(
            context.state, &self.next, &self.result,
            &self.envelope, files,
        )
    }

    pub(crate) fn require_candidate(
        &self, candidate: &crate::journal::Candidate, files: &WorkingBatchHandoff,
        material: &PreparedDirectoryMaterial,
    ) -> Result<()> {
        files.require_candidate(candidate).map_err(DirectoryPublicationError::Engine)?;
        if candidate.request_digest != self.result.request_digest
            || candidate.sequence != self.next.sequence
            || self.next.operations.get(&candidate.operation_id) != Some(&self.result)
        {
            return Err(DirectoryPublicationError::Candidate);
        }
        self.require_material(files, material)
    }
}
