//! The exact retained Added occurrence issues the shared FULL preparation.
use crate::business::file_draft::{DraftError as Failure, invalid};
use crate::container_storage::{HeldUnbornReader, ObservedContainerReplica, PreparedUnbornFull};
use crate::engine::{MutationPlan, PlanningContext};
use crate::persist::snapshot_worktree::added_commit::{AddedCommit, AddedFirst};
use crate::persist::snapshot_worktree::{PublicationEncoding, Record};
use crate::{Engine, Error, OperationResult, Result, State, ValidatedConfig};
use std::sync::Arc;
use storage_contract::Principal;

pub(crate) struct WorktreeFirstSource {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
    snapshot: Option<String>,
    slot: u64,
    reader: Arc<HeldUnbornReader>,
}
impl WorktreeFirstSource {
    pub(crate) fn reader(&self) -> &Arc<HeldUnbornReader> {
        &self.reader
    }
    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Arc::ptr_eq(&self.state, context.state)
            || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || self.snapshot != *context.snapshot
            || self.slot != context.next_slot
        {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        self.reader.require_source()
    }
    fn capture(engine: &Engine, plan: &AddedCommit, principal: &Principal) -> Result<Arc<Self>> {
        use crate::business::file_draft::worktree::{failure, selected};
        let context = engine.planning_context();
        context.require_write()?;
        context
            .snapshot_receipts()
            .map_err(failure)?
            .require_added_first(plan, principal)
            .map_err(failure)?;
        let record = selected(context.state, &plan.added.target)?;
        if record.created_by != plan.added.birth
            || record.working.unborn() != Some(&plan.added.working)
        {
            return Err(invalid());
        }
        let disk = context
            .config
            .disks
            .iter()
            .find(|disk| disk.disk_id == record.owner_disk.to_string())
            .ok_or_else(invalid)?;
        let source = crate::engine::fresh_layout::containers::published_source(
            context.config,
            context.disk_resources,
            context.state,
            plan.added.target.file,
            disk,
        )?;
        let observed = Arc::new(ObservedContainerReplica::observe(source)?);
        let reader = Arc::new(observed.open_unborn_working(context.config.max_file_bytes)?);
        let source = Arc::new(Self {
            state: Arc::clone(context.state),
            config: context.config.clone(),
            resources: Arc::clone(context.disk_resources),
            snapshot: context.snapshot.clone(),
            slot: context.next_slot,
            reader,
        });
        source.require_context(&context)?;
        Ok(source)
    }
}

impl Engine {
    pub(crate) fn publish_added_first(
        &mut self,
        plan: &AddedCommit,
        principal: &Principal,
        ledger: &crate::directory_worktree::ledger::HeldWorktreeLedger<'_>,
    ) -> Result<(Arc<PreparedUnbornFull>, AddedFirst, OperationResult)> {
        let selected = WorktreeFirstSource::capture(self, plan, principal)?;
        let material = PreparedUnbornFull::prepare_worktree(&self.planning_context(), selected)?;
        let reservation = Record::ReserveAddedCommit(plan.clone())
            .commitment()
            .map_err(|source| {
                crate::business::file_draft::worktree::failure(
                    crate::snapshot_worktree::RegistrationError::Codec(source),
                )
            })?;
        let input = crate::persist::snapshot_worktree::added_commit::first_request(
            plan.request.operation_id,
            reservation,
        )?;
        let mut first = None;
        let planned = self.plan_mutation_digest(
            crate::persist::snapshot_worktree::ADDED_FIRST,
            principal.as_str(),
            plan.file_operation,
            input,
            None,
            |engine, next, prepared| {
                let context = engine.planning_context();
                let content = crate::business::file_draft::worktree::first::build(
                    &context, next, prepared, plan, principal, &material,
                )?;
                let outcome = AddedFirst {
                    reserve: plan.request.operation_id,
                    reservation,
                    content,
                    full: material.worktree_evidence()?,
                };
                let file = outcome
                    .file_result(plan)
                    .map_err(crate::business::file_draft::worktree::failure)?;
                ledger
                    .check_added_publication(&plan.commit_event(), plan.publication(), &file)
                    .map_err(|source| {
                        crate::business::file_draft::worktree::failure(
                            crate::snapshot_worktree::RegistrationError::Ledger(source),
                        )
                    })?;
                let record = Record::PublishAddedFile(outcome.clone());
                let encoded = PublicationEncoding::new(&record).map_err(|source| {
                    crate::business::file_draft::worktree::failure(
                        crate::snapshot_worktree::RegistrationError::Codec(source),
                    )
                })?;
                first = Some((outcome, record));
                Ok(encoded.result())
            },
        )?;
        let MutationPlan::Prepared {
            next,
            prepared,
            result,
        } = planned
        else {
            return Err(invalid());
        };
        let (first, record) = first.ok_or_else(invalid)?;
        material.require_context(&self.planning_context())?;
        self.snapshot_receipts()
            .map_err(crate::business::file_draft::worktree::failure)?
            .require_planned(&next, &result, principal, &record)
            .map_err(crate::business::file_draft::worktree::failure)?;
        self.persist(*next, prepared, result.clone())?;
        material.require_source()?;
        Ok((material, first, result))
    }
}
