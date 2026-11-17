//! The actual held-set producer for one mixed working-file projection.
//! It has no persist/clear method; the directory transaction owns one decision.
use crate::engine::file_draft::{SelectedWorkingSources, require_initial};
use crate::business::file_draft::{DraftError as Failure, batch, invalid};
use crate::engine::{PlanningContext, Prepared};
use crate::{Error, Result, State};
use crate::container_storage::{PreparedUnbornFull, ReadWorkingDigest};
use std::sync::Arc;
use storage_contract::{FileId, OperationId, Principal, Sequence};

pub(crate) use batch::{
    WorkingBatchItem, WorkingBatchOrigin, WorkingBatchRequest, WorkingBatchResult,
    WorkingBatchTrace,
};

/// Neither descriptive trace replay nor independent singleton plans construct
/// this product. All items retain one complete selected occurrence until the
/// eventual physical preparation and combined decision finish.
pub(crate) struct PreparedWorkingBatch {
    sources: Arc<SelectedWorkingSources>,
    request: WorkingBatchRequest,
    projection: batch::WorkingBatchProjection,
    first: Vec<Option<Arc<PreparedUnbornFull>>>,
    prepared: Vec<Prepared>,
}

impl PreparedWorkingBatch {
    pub(crate) fn prepare(
        context: &PlanningContext<'_>, sources: Arc<SelectedWorkingSources>,
        operation: OperationId, principal: &Principal, message: &str,
    ) -> Result<Self> {
        context.require_write()?;
        sources.require_context(context)?;
        if context.current_namespaces.is_some() || context.config.generation().is_some() {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        if context.state.operations.contains_key(&operation) { return Err(invalid()); }
        crate::directory_conversion::require_import_not_stopped(context.state, operation)?;
        let first = sources.sources().first()
            .ok_or(Error::FileDraft(Failure::WorkingSelection))?;
        if sources.sources().iter().any(|source| {
            source.target().repository != first.target().repository
                || source.record().owner_disk != first.record().owner_disk
        }) {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        let selected = sources.sources().iter().map(|source| source.selection().clone()).collect();
        let request = WorkingBatchRequest::new(
            operation, principal.clone(), message.into(), selected,
        );
        // Source and all namespace/owner admissions precede Version allocation.
        // The batch planner separately completes every selected CAS/category check.
        for source in sources.sources() { require_initial(context, source.target())?; }
        let (projection, prepared, first) = batch::build(context, &sources, &request)?;
        let result = Self { sources, request, projection, first, prepared };
        result.require_context(context)?;
        result.require_prepared(&result.prepared)?;
        Ok(result)
    }

    pub(crate) fn request(&self) -> &WorkingBatchRequest { &self.request }
    pub(crate) fn sources(&self) -> &Arc<SelectedWorkingSources> { &self.sources }
    pub(crate) fn state(&self) -> &State { self.projection.state() }
    pub(crate) fn items(&self) -> &[WorkingBatchItem] { self.projection.items() }
    pub(crate) fn trace(&self) -> &WorkingBatchTrace { self.projection.trace() }

    pub(crate) fn first_reads(&self) -> Result<Vec<Option<ReadWorkingDigest<'_>>>> {
        self.first.iter().map(|material| material.as_ref().map(|full| full.digest()).transpose())
            .collect()
    }
    pub(crate) fn prepared(&self) -> &[Prepared] { &self.prepared }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        context.require_write()?;
        self.sources.require_context(context)?;
        for material in self.first.iter().flatten() {
            material.require_context(context)?;
        }
        if context.current_namespaces.is_some() || context.config.generation().is_some() {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        Ok(())
    }

    pub(crate) fn require_selection(&self, sources: &Arc<SelectedWorkingSources>) -> Result<()> {
        if !Arc::ptr_eq(&self.sources, sources) {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        Ok(())
    }

    /// The later physical gate consumes precisely these Version obligations,
    /// not equal-byte files, a second picker, or an external path substitution.
    pub(crate) fn require_prepared(&self, prepared: &[Prepared]) -> Result<()> {
        require_prepared(self.items(), &self.sources, &self.first, prepared)
    }

    /// A deterministic replay check of this same still-held product. It is not
    /// cold source admission; the new native directory mode must supply that.
    pub(crate) fn require_reconstruction(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.require_context(context)?;
        let replay = batch::replay(
            context.state, context.config, &self.request, Sequence::new(context.next_slot),
            self.trace(), &self.first_reads()?,
        )?;
        if !crate::publication_shape::same_transition(
            context.state, replay.state(), self.state(),
        ) {
            return Err(invalid());
        }
        self.require_prepared(&self.prepared)?;
        self.require_context(context)
    }
}

/// A one-way transfer from the actual producer. The native directory envelope
/// owns the only final State/result. This guard retains the complete FILE
/// projection and selected FD occurrence while that envelope is prepared.
/// Descriptive cold replay cannot construct this live product.
pub(crate) struct WorkingBatchHandoff {
    sources: Arc<SelectedWorkingSources>,
    request: WorkingBatchRequest,
    projection: batch::WorkingBatchProjection,
    first: Vec<Option<Arc<PreparedUnbornFull>>>,
}

impl PreparedWorkingBatch {
    pub(crate) fn into_publication(
        self,
        context: &PlanningContext<'_>,
    ) -> Result<(State, Vec<Prepared>, WorkingBatchHandoff)> {
        self.require_reconstruction(context)?;
        let Self { sources, request, projection, first, prepared } = self;
        let next = projection.state().clone();
        let source = WorkingBatchHandoff { sources, request, projection, first };
        Ok((next, prepared, source))
    }
}

impl WorkingBatchHandoff {
    pub(crate) fn require_selection(&self, sources: &Arc<SelectedWorkingSources>) -> Result<()> {
        if !Arc::ptr_eq(&self.sources, sources) {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        Ok(())
    }

    pub(crate) fn request(&self) -> &WorkingBatchRequest { &self.request }
    pub(crate) fn state(&self) -> &State { self.projection.state() }
    pub(crate) fn items(&self) -> &[WorkingBatchItem] { self.projection.items() }
    pub(crate) fn trace(&self) -> &WorkingBatchTrace { self.projection.trace() }

    pub(crate) fn first_material(&self, file: FileId) -> Option<&Arc<PreparedUnbornFull>> {
        self.items().iter().position(|item| item.target().file == file)
            .and_then(|index| self.first[index].as_ref())
    }

    pub(crate) fn first_reads(&self) -> Result<Vec<Option<ReadWorkingDigest<'_>>>> {
        self.first.iter().map(|material| material.as_ref().map(|full| full.digest()).transpose())
            .collect()
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        context.require_write()?;
        self.sources.require_context(context)?;
        for material in self.first.iter().flatten() {
            material.require_context(context)?;
        }
        if context.current_namespaces.is_some() || context.config.generation().is_some() {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        Ok(())
    }

    pub(crate) fn require_prepared(&self, prepared: &[Prepared]) -> Result<()> {
        require_prepared(self.items(), &self.sources, &self.first, prepared)
    }

    /// This closes only the FILE obligations. The directory's final native
    /// product separately binds its request digest, snapshot and node feedback.
    pub(crate) fn require_candidate(&self, candidate: &crate::journal::Candidate) -> Result<()> {
        if candidate.operation_id != self.request.operation()
            || candidate.sequence != self.state().sequence
        {
            return Err(invalid());
        }
        self.projection.require_versions(&candidate.versions)
    }

    /// Recheck the held sources after PREPARE without treating the pipeline's
    /// own slot advance as a fresh selection. The enclosing native product
    /// must still bind its candidate/configuration at that decision boundary.
    pub(crate) fn require_material(&self) -> Result<()> {
        for source in self.sources.sources() {
            source.require_source()?;
        }
        for material in self.first.iter().flatten() {
            material.require_source()?;
        }
        Ok(())
    }
}

fn require_prepared(
    items: &[WorkingBatchItem],
    sources: &Arc<SelectedWorkingSources>,
    first: &[Option<Arc<PreparedUnbornFull>>],
    prepared: &[Prepared],
) -> Result<()> {
    let mut supplied = prepared.iter();
    for (index, item) in items.iter().enumerate() {
        if let WorkingBatchResult::Published(content) = item.result() {
            let actual = supplied.next().ok_or_else(invalid)?;
            if actual.version != content.version || actual.placement != content.placement {
                return Err(invalid());
            }
            match item.origin() {
                WorkingBatchOrigin::Born(_) => {
                    actual.source.require_selected_working(sources, index)?;
                }
                WorkingBatchOrigin::First { .. } => {
                    let full = first[index].as_ref().ok_or_else(invalid)?;
                    if !Arc::ptr_eq(actual.source.require_working_full()?, full) {
                        return Err(invalid());
                    }
                    full.require_selection(sources, index)?;
                }
            }
        }
    }
    if supplied.next().is_some() {
        return Err(invalid());
    }
    Ok(())
}
