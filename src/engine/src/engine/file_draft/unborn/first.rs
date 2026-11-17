//! Explicit single-file first commit. Ingress and ordinary saves never call it.
//! Construction remains a separate typed preparation; the existing native
//! decision path alone can install its whole-State result and clear the draft.
use crate::business::file_draft::{self as business, DraftError as Failure, PublishedContent};
use crate::container_storage::PreparedUnbornFull;
use crate::engine::{MutationPlan, PlanningContext, Prepared};
use crate::{Engine, Error, OperationResult, Result, State};
use std::sync::Arc;
use storage_contract::file_draft::{FileWorkingState, WorkingSelection};
use storage_contract::{OperationId, Principal};

/// Real facts from the one Version constructor and canonical whole-State kernel.
/// This is not FirstPublished: no candidate or native receipt has been decided.
pub(crate) struct PreparedFirstFile {
    material: Arc<PreparedUnbornFull>,
    projection: business::unborn::Projection,
    prepared: Vec<Prepared>,
    content: PublishedContent,
}

impl PreparedFirstFile {
    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.material.require_context(context)
    }

    pub(crate) fn state(&self) -> &State { self.projection.state() }
    pub(crate) fn content(&self) -> &PublishedContent { &self.content }
    pub(crate) fn prepared(&self) -> &[Prepared] { &self.prepared }

    pub(crate) fn working_after(&self) -> Result<&FileWorkingState> {
        self.projection.state().file_containers.get(&self.projection.target().file)
            .map(|record| &record.working).ok_or_else(business::invalid)
    }
}

impl Engine {
    /// Exact Existing receipts are resolved before re-opening an old Unborn
    /// selection. A retry after first commit must not materialize a second FULL.
    pub fn commit_unborn_working(
        &mut self,
        request: &WorkingSelection,
        principal: &Principal,
        operation: OperationId,
        message: &str,
    ) -> Result<OperationResult> {
        use crate::persist::file_draft::unborn_first as codec;
        let plan = self.plan_mutation_digest(
            codec::KIND, principal.as_str(), operation, codec::request(request, message)?, None,
            |engine, next, versions| {
                let first = engine.prepare_unborn_first_file(
                    request, principal, operation, message,
                )?;
                first.require_context(&engine.planning_context())?;
                let encoded = codec::encode(request, message, &first.content)?;
                *next = first.projection.into_state();
                versions.extend(first.prepared);
                Ok(encoded)
            },
        )?;
        match plan {
            MutationPlan::Existing(result) => Ok(result),
            MutationPlan::Prepared { next, prepared, result } => {
                self.persist(*next, prepared, result.clone())?;
                Ok(result)
            }
        }
    }

    /// Explicit single-file preparation only. It returns no operation result,
    /// installs no historical state and never acknowledges an ordinary save.
    pub(crate) fn prepare_unborn_first_file(
        &self,
        request: &WorkingSelection,
        principal: &Principal,
        operation: OperationId,
        message: &str,
    ) -> Result<PreparedFirstFile> {
        let context = self.planning_context();
        context.require_write()?;
        if context.state.operations.contains_key(&operation) {
            // Idempotent publication belongs to the native request gate, not a
            // second materialization of a previously decided operation.
            return Err(business::invalid());
        }
        if context.current_namespaces.is_some()
            || context.config.generation().is_some()
        {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        crate::directory_conversion::require_import_not_stopped(context.state, operation)?;
        let selected = Arc::new(self.select_working_sources(std::slice::from_ref(request))?);
        let source = &selected.sources()[0];
        source.unborn_reader()?;
        let (primary, copies) = context.new_file_placement_for_file(
            &request.target.repository.to_string(), Some(&request.target.file.to_string()),
            request.target.path.as_str(),
        )?;
        let disk = context.disk(&primary)?;
        if copies != 1 || disk.level != 1 || disk.upstream_slot_id.is_some()
            || disk.disk_id.parse::<storage_contract::DiskId>()? != source.record().owner_disk
        {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        business::trace_limit(context.state, &request.target)?;
        let material = PreparedUnbornFull::prepare(&context, selected, 0)?;
        let (projection, prepared, content) = business::unborn::build_first_content(
            &context, &request.target, request.expected.revision, message, principal,
            operation, &material,
        )?;
        let result = PreparedFirstFile { material, projection, prepared, content };
        result.require_context(&context)?;
        Ok(result)
    }
}
