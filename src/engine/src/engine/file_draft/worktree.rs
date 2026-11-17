//! Hot worktree save input is issued only from the same held extent preparation.
use crate::business::file_draft::{DraftError as Failure, invalid};
use crate::container_storage::PreparedWorkingSave;
use crate::engine::{PlanningContext, file_draft::WorkingPublicationInput};
use crate::persist::snapshot_worktree::Record;
use crate::{Engine, Error, OperationResult, Result, State};
use std::sync::Arc;
use storage_contract::FileId;

pub(in crate::engine) struct WorktreePublicationInput {
    prepared: Arc<PreparedWorkingSave>,
    planned: State,
    result: OperationResult,
    slot: u64,
}
impl WorktreePublicationInput {
    pub(in crate::engine::file_draft) fn file(&self) -> FileId {
        self.prepared.manifest().file()
    }
    pub(in crate::engine::file_draft) fn require_context(
        &self,
        context: &PlanningContext<'_>,
    ) -> Result<()> {
        if self.slot != context.next_slot {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        self.prepared.require_context(context)
    }
    pub(in crate::engine::file_draft) fn require_next(
        &self,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        if &self.planned != next || &self.result != result {
            return Err(invalid());
        }
        Ok(())
    }
}
impl Engine {
    pub(crate) fn persist_worktree_file_save(
        &mut self,
        next: State,
        result: OperationResult,
        prepared: Arc<PreparedWorkingSave>,
    ) -> Result<()> {
        let context = self.planning_context();
        prepared.require_context(&context)?;
        let decoded = crate::persist::snapshot_worktree::decode(&result)
            .map_err(|source| {
                crate::business::file_draft::worktree::failure(
                    crate::snapshot_worktree::RegistrationError::Codec(source),
                )
            })?
            .ok_or_else(invalid)?;
        let Record::SaveFile(plan) = decoded.record else {
            return Err(invalid());
        };
        if prepared.worktree_request(&plan.request.target, plan.request.expected)? != plan.request {
            return Err(invalid());
        }
        let input = WorktreePublicationInput {
            prepared,
            planned: next.clone(),
            result: result.clone(),
            slot: context.next_slot,
        };
        self.persist_working_publication(next, result, WorkingPublicationInput::Worktree(input))
    }
}
