//! Ordinary Manual saves reuse the same native publisher and immutable extent producer.
use crate::business::file_draft::{self as business, DraftError as Failure, Outcome};
use crate::container_storage::{HeldManualReader, ObservedContainerReplica, PreparedWorkingSave};
use crate::engine::file_draft::{WorkingPublicationInput, require_initial};
use crate::engine::fresh_layout::containers::published_source;
use crate::engine::{MutationPlan, PlanningContext};
use crate::persist::file_draft as codec;
use crate::{Engine, Error, OperationResult, Result, State};
use std::sync::Arc;
use storage_contract::file_draft::DraftLocation;
use storage_contract::{FileId, OperationId, Principal};

pub(in crate::engine) struct IncrementalPublicationInput {
    material: Arc<PreparedWorkingSave>,
    next: State,
    result: OperationResult,
    slot: u64,
}

impl IncrementalPublicationInput {
    pub(super) fn file(&self) -> FileId {
        self.material.manifest().file()
    }
    pub(super) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if context.next_slot != self.slot {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        self.material.require_context(context)
    }
    pub(super) fn require_next(&self, next: &State, result: &OperationResult) -> Result<()> {
        if &self.next != next || &self.result != result {
            return Err(business::invalid());
        }
        Ok(())
    }
}

impl Engine {
    pub(super) fn open_manual_working(&self, target: &DraftLocation) -> Result<HeldManualReader> {
        let context = self.planning_context();
        require_initial(&context, target)?;
        let selection = business::resolve(context.state, target)?;
        let record = context
            .state
            .file_containers
            .get(&target.file)
            .ok_or_else(business::invalid)?;
        let disk = context
            .config
            .disks
            .iter()
            .find(|disk| disk.disk_id == record.owner_disk.to_string())
            .ok_or(Error::FileDraft(Failure::UnsupportedScope))?;
        let source = published_source(
            context.config,
            context.disk_resources,
            context.state,
            target.file,
            disk,
        )?;
        let scope = Arc::new(ObservedContainerReplica::observe(source)?);
        let version = context
            .state
            .versions
            .get(&selection.base.version_id().to_string())
            .ok_or_else(business::invalid)?;
        HeldManualReader::capture(scope, selection.base, version)
    }

    pub(super) fn save_incremental_working(
        &mut self,
        request: &codec::RangeSaveRequest,
        principal: &Principal,
        operation: OperationId,
        material: Arc<PreparedWorkingSave>,
    ) -> Result<OperationResult> {
        let plan = self.plan_mutation_digest(
            "file.draft.write",
            principal.as_str(),
            operation,
            codec::request(request)?,
            None,
            |engine, next, _| {
                material.require_context(&engine.planning_context())?;
                require_initial(&engine.planning_context(), &request.target)?;
                if material.manual_command(&request.target)? != *request {
                    return Err(business::invalid());
                }
                let (payload, manifest) = material.manual_material()?;
                manifest
                    .material()
                    .validate(engine.config.max_file_bytes)
                    .map_err(|source| Error::FileDraft(Failure::WorkingManifest(source)))?;
                let working = business::plan_incremental(
                    engine.state(),
                    request,
                    payload,
                    &manifest,
                    operation,
                )?;
                business::apply(next, request.target.file, working.clone())?;
                codec::encode(Outcome {
                    request: codec::Request::Ranges(request.clone()),
                    working,
                    content: None,
                })
            },
        )?;
        match plan {
            MutationPlan::Existing(result) => Ok(result),
            MutationPlan::Prepared {
                next,
                prepared,
                result,
            } => {
                if !prepared.is_empty() {
                    return Err(business::invalid());
                }
                let input = IncrementalPublicationInput {
                    material,
                    next: (*next).clone(),
                    result: result.clone(),
                    slot: self.planning_context().next_slot,
                };
                self.persist_working_publication(
                    *next,
                    result.clone(),
                    WorkingPublicationInput::Incremental(input),
                )?;
                Ok(result)
            }
        }
    }
}

/// Carries the actual explicit merged read even when the equal-base decision
/// creates no Prepared version. A serialized hash cannot supply this input.
pub(in crate::engine) struct CommittedDraftInput {
    before: Arc<State>,
    read: Arc<crate::container_storage::ReadDraftDigest>,
    next: State,
    result: OperationResult,
}

impl CommittedDraftInput {
    pub(super) fn new(
        context: &PlanningContext<'_>, next: &State, result: &OperationResult,
        read: Arc<crate::container_storage::ReadDraftDigest>,
    ) -> Result<Self> {
        let value = Self { before: Arc::clone(context.state), read,
            next: next.clone(), result: result.clone() };
        value.require_context(context)?;
        Ok(value)
    }
    pub(super) fn file(&self) -> FileId { self.read.file() }
    pub(super) fn read(&self) -> &Arc<crate::container_storage::ReadDraftDigest> { &self.read }
    pub(super) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Arc::ptr_eq(&self.before, context.state) { return Err(business::invalid()); }
        let record = context.state.file_containers.get(&self.file()).ok_or_else(business::invalid)?;
        self.read.require_record(self.file(), record)
    }
    pub(super) fn require_next(&self, next: &State, result: &OperationResult) -> Result<()> {
        if &self.next != next || &self.result != result { return Err(business::invalid()); }
        self.read.require_source()
    }
}
