//! One durable save consumes the actual immutable extent preparation. Ordinary
//! transport frames do not call a publication or advance SaveGeneration.
use crate::business::file_draft::{self as business, DraftError as Failure};
use crate::container_storage::{PreparedUnbornSave, UnbornWriteSession};
use crate::engine::file_draft::WorkingPublicationInput;
use crate::engine::{MutationPlan, PlanningContext};
use crate::persist::file_draft::unborn_save as codec;
use crate::{Engine, Error, OperationResult, Result, State};
use std::sync::Arc;
use storage_contract::file_draft::{DraftLocation, UnbornSaveRequest};
use storage_contract::{FileId, OperationId, Principal};

/// Only the planner's successful canonical projection can bind prepared extents
/// to a complete next State. The original descriptors survive native capture.
pub(in crate::engine) struct UnbornPublicationInput {
    prepared: Arc<PreparedUnbornSave>,
    planned: State,
    result: OperationResult,
    slot: u64,
}

impl UnbornPublicationInput {
    fn planned(
        context: &PlanningContext<'_>,
        next: &State,
        result: &OperationResult,
        prepared: Arc<PreparedUnbornSave>,
    ) -> Result<Self> {
        prepared.require_context(context)?;
        let decoded = codec::decode(result)?;
        let request = prepared.request(&decoded.request().target);
        if &request != decoded.request() || result.operation_id != request.manifest.operation() {
            return Err(business::invalid());
        }
        Ok(Self {
            prepared, planned: next.clone(), result: result.clone(), slot: context.next_slot,
        })
    }

    pub(in crate::engine::file_draft) fn file(&self) -> FileId {
        self.prepared.manifest().file()
    }

    pub(in crate::engine::file_draft) fn require_context(
        &self, context: &PlanningContext<'_>,
    ) -> Result<()> {
        if context.next_slot != self.slot {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        self.prepared.require_context(context)
    }

    pub(in crate::engine::file_draft) fn require_next(
        &self, next: &State, result: &OperationResult,
    ) -> Result<()> {
        // Physical feedback cannot change an existing Unborn occurrence during
        // Save. Its exact new body was already produced by the canonical kernel.
        if &self.result != result || self.planned != *next {
            return Err(Error::FileDraft(Failure::Publication));
        }
        Ok(())
    }
}

impl Engine {
    pub(crate) fn begin_unborn_write(
        &self, target: &DraftLocation, operation: OperationId,
    ) -> Result<UnbornWriteSession> {
        UnbornWriteSession::begin(
            &self.planning_context(), self.open_unborn_working(target)?, operation,
        )
    }

    fn plan_unborn_save(
        &self,
        request: &UnbornSaveRequest,
        principal: &Principal,
        prepared: Option<&PreparedUnbornSave>,
    ) -> Result<MutationPlan> {
        self.plan_mutation_digest(
            codec::KIND, principal.as_str(), request.manifest.operation(),
            codec::request(request)?, None,
            |engine, next, _| {
                let prepared = prepared.ok_or_else(business::invalid)?;
                let projection = prepared.project(&engine.planning_context(), &request.target)?;
                if prepared.request(&request.target) != *request {
                    return Err(business::invalid());
                }
                *next = projection.into_state();
                codec::encode(request)
            },
        )
    }

    /// Existing is resolved from the exact request before the old preparation's
    /// State occurrence or revision is checked. A retry never makes a new save.
    /// After restart, the original request alone can retrieve Existing; without
    /// a real preparation it cannot create a new receipt or adopt any extent.
    pub(crate) fn save_unborn_working(
        &mut self,
        request: &UnbornSaveRequest,
        principal: &Principal,
        prepared: Option<Arc<PreparedUnbornSave>>,
    ) -> Result<OperationResult> {
        match self.plan_unborn_save(request, principal, prepared.as_deref())? {
            MutationPlan::Existing(result) => Ok(result),
            MutationPlan::Prepared { next, prepared: versions, result } => {
                if !versions.is_empty() { return Err(business::invalid()); }
                let prepared = prepared.ok_or_else(business::invalid)?;
                let input = UnbornPublicationInput::planned(
                    &self.planning_context(), &next, &result, prepared,
                )?;
                self.persist_working_publication(
                    *next, result.clone(), WorkingPublicationInput::Unborn(input),
                )?;
                Ok(result)
            }
        }
    }
}
