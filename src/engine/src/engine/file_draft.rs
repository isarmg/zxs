//! Shared container draft operations. Transport adapters supply authenticated
//! principals; every call still resolves the exact selected container and source.
use crate::business::file_draft::{self as business, DraftError as Failure, Outcome};
use crate::persist::file_draft as codec;
use crate::{Engine, Error, OperationResult, ReadFile, Result, State, ValidatedConfig};
use super::{MutationPlan, PlanningContext};
use std::sync::Arc;
use storage_contract::file_draft::{
    DraftBase, DraftCommand, DraftLocation, DraftPayload, DraftPayloadId, FileWorkingState,
};
use storage_contract::{FileId, OperationId, Principal};

mod unborn;
mod native_io;
pub(crate) mod worktree;
mod incremental;
pub use native_io::{NativeCurrentEntry, NativeWorkingFile, NativeWorkingTarget};
mod selected;
mod batch;
pub(crate) use batch::{
    PreparedWorkingBatch, WorkingBatchHandoff, WorkingBatchOrigin, WorkingBatchResult,
};
mod input;
pub(super) use input::WorkingPublicationInput;
pub(crate) use selected::{SelectedWorkingSource, SelectedWorkingSources};

pub(crate) use business::{validate_working_record, verify_working_transition};

fn require_initial(context: &PlanningContext<'_>, target: &DraftLocation) -> Result<()> {
    if context.current_namespaces.is_some() || context.config.generation().is_some() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let record = context.state.file_containers.get(&target.file).ok_or_else(business::invalid)?;
    if record.replicas.len() != 1 || !record.replicas.contains_key(&record.owner_disk) {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    if target.path.parent().is_some() {
        let occurrence = business::ordinary::OrdinaryOccurrence::capture(context.state, target.file, record)?;
        occurrence.require_target(target)?;
        let index = context.snapshot_receipts()
            .map_err(|source| Error::FileDraft(Failure::NestedRegistration(Box::new(source.into()))))?;
        if index.contains_nested_draft_host(target.repository, record.namespace.branch, &occurrence.parents) {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
    }
    // This is the existing published root-origin issuer, not a path lookup or
    // direct Config -> namespace grant. Actual container use rechecks its entity.
    context.namespace_host_root(target.repository, record.namespace.branch)?.common()?;
    Ok(())
}

/// Owned plan input for one non-Version BODY. No caller can mint this by
/// supplying a FileId/hash/path or substitute another identical State allocation.
pub(crate) struct DraftPublicationInput {
    before: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
    snapshot: Option<String>,
    slot: u64,
    planned: State,
    receipt: OperationResult,
    target: DraftLocation,
    file: FileId,
    payload: DraftPayload,
    source: ReadFile,
}
impl DraftPublicationInput {
    fn planned(
        context: &PlanningContext<'_>,
        next: &State,
        result: &OperationResult,
        command: &DraftCommand,
        source: ReadFile,
    ) -> Result<Self> {
        if !matches!(command, DraftCommand::Write { .. }) {
            return Err(business::invalid());
        }
        let file = command.target().file;
        let payload = *next.file_containers.get(&file).and_then(|record| record.working.draft())
            .ok_or_else(business::invalid)?.1;
        Ok(Self {
            before: Arc::clone(context.state), config: context.config.clone(),
            resources: Arc::clone(context.disk_resources), snapshot: context.snapshot.clone(),
            slot: context.next_slot, planned: next.clone(), receipt: result.clone(),
            target: command.target().clone(), file, payload, source,
        })
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Arc::ptr_eq(&self.before, context.state)
            || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || self.snapshot != *context.snapshot || self.slot != context.next_slot
        {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        require_initial(context, &self.target)
    }

    pub(crate) fn require_next(&self, next: &State, result: &OperationResult) -> Result<()> {
        if &self.receipt != result
            || !crate::publication_shape::same_transition(&self.before, &self.planned, next)
        {
            return Err(Error::FileDraft(Failure::Publication));
        }
        Ok(())
    }

    pub(crate) fn file(&self) -> FileId {
        self.file
    }
    pub(crate) fn payload(&self) -> DraftPayload {
        self.payload
    }
    pub(crate) fn source(&mut self) -> &mut ReadFile {
        &mut self.source
    }
}

/// Descriptive FILE working state plus its current read binding; not transport
/// authorization, an earlier request's receipt, or a capability to publish.
pub struct SelectedFileWorking<'a> {
    engine: &'a Engine,
    file: FileId,
    base: DraftBase,
    working: &'a FileWorkingState,
}

impl SelectedFileWorking<'_> {
    pub fn base(&self) -> DraftBase { self.base }

    pub fn working(&self) -> &FileWorkingState { self.working }

    /// A stale saved base remains visible as metadata so the user can
    /// explicitly discard it. Never overlay or serve that body as current.
    pub fn selected_payload(&self) -> Result<Option<DraftPayload>> {
        match self.working.draft() {
            Some((base, _)) if *base != self.base => Err(Error::FileDraft(Failure::BaseChanged)),
            Some((_, payload)) => Ok(Some(*payload)),
            None => Ok(None),
        }
    }

    pub fn open_draft(&self) -> Result<ReadFile> {
        if self.selected_payload()?.is_none() {
            return Err(Error::FileDraft(Failure::DraftRequired));
        }
        self.engine.planning_context().draft_payload_source(self.file)?.open()
    }
}

impl Engine {
    pub fn file_draft_base(&self, target: &DraftLocation) -> Result<DraftBase> {
        require_initial(&self.planning_context(), target)?;
        Ok(business::resolve(self.state(), target)?.base)
    }

    /// Freeze the current exact occurrence together with the saved record (which
    /// may show an older base). The borrow cannot cross an Engine mutation or
    /// reopen another target's body.
    pub fn selected_file_working(&self, target: &DraftLocation) -> Result<SelectedFileWorking<'_>> {
        let base = self.file_draft_base(target)?;
        let record = self.state.file_containers.get(&target.file).ok_or_else(business::invalid)?;
        Ok(SelectedFileWorking { engine: self, file: target.file, base, working: &record.working })
    }

    pub fn file_working_state(&self, target: &DraftLocation) -> Result<FileWorkingState> {
        Ok(self.selected_file_working(target)?.working().clone())
    }

    pub fn open_file_draft(&self, target: &DraftLocation) -> Result<ReadFile> {
        self.selected_file_working(target)?.open_draft()
    }

    /// The future close/flush route uses this writer, not the explicit Commit
    /// command. Only this draft payload and its working revision are published.
    pub fn write_file_draft(
        &mut self,
        command: &DraftCommand,
        principal: &Principal,
        operation: OperationId,
        source: ReadFile,
    ) -> Result<OperationResult> {
        let DraftCommand::Write { content_hash, logical_size, .. } = command else {
            return Err(business::invalid());
        };
        let plan = self.plan_mutation_digest(
            command.kind(), principal.as_str(), operation, codec::request(command)?, None,
            |engine, next, _| {
                require_initial(&engine.planning_context(), command.target())?;
                if *logical_size > engine.config.max_file_bytes {
                    return Err(Error::FileDraft(Failure::Size));
                }
                let payload = DraftPayload::new(
                    DraftPayloadId::generate()?, *content_hash, *logical_size,
                );
                let working = business::plan_change(engine.state(), command, Some(payload))?;
                business::apply(next, command.target().file, working.clone())?;
                codec::encode(Outcome { request: codec::Request::Command(command.clone()), working, content: None })
            },
        )?;
        match plan {
            MutationPlan::Existing(receipt) => Ok(receipt),
            MutationPlan::Prepared { next, prepared, result } => {
                if !prepared.is_empty() { return Err(business::invalid()); }
                let input = DraftPublicationInput::planned(
                    &self.planning_context(), &next, &result, command, source,
                )?;
                self.persist_draft_publication(*next, result.clone(), input)?;
                Ok(result)
            }
        }
    }

    /// Mode/discard are metadata-only; commit opens the already published draft
    /// through its retained container source and runs the real version builder.
    pub fn change_file_working(
        &mut self,
        command: &DraftCommand,
        principal: &Principal,
        operation: OperationId,
    ) -> Result<OperationResult> {
        if matches!(command, DraftCommand::Write { .. }) { return Err(business::invalid()); }
        let mut committed = None;
        let plan = self.plan_mutation_digest(
            command.kind(), principal.as_str(), operation, codec::request(command)?, None,
            |engine, next, prepared| {
                let context = engine.planning_context();
                require_initial(&context, command.target())?;
                let working = business::plan_change(engine.state(), command, None)?;
                let read = if matches!(command, DraftCommand::Commit { .. })
                    && context.state.file_containers.get(&command.target().file)
                        .and_then(|record| record.working.incremental()).is_some()
                {
                    Some(crate::container_storage::ReadDraftDigest::capture(
                        Arc::new(engine.open_manual_working(command.target())?),
                        context.config.checksum_chunk_bytes,
                    )?)
                } else { None };
                let content = match command {
                    DraftCommand::Commit { .. } => {
                        if business::draft_matches_base(engine.state(), command, read.as_deref())? {
                            None
                        } else {
                            Some(business::build_content(
                                &context, next, prepared, command, principal.as_str(), operation, read.as_deref(),
                            )?)
                        }
                    }
                    DraftCommand::SetMode { .. } | DraftCommand::Discard { .. } => None,
                    DraftCommand::Write { .. } => return Err(business::invalid()),
                };
                // Clear the draft only after the equal-base check or the content
                // constructor succeeds. Errors leave the published draft intact.
                business::apply(next, command.target().file, working.clone())?;
                committed = read;
                codec::encode(Outcome { request: codec::Request::Command(command.clone()), working, content })
            },
        )?;
        match plan {
            MutationPlan::Existing(receipt) => Ok(receipt),
            MutationPlan::Prepared { next, prepared, result } => {
                match committed {
                    Some(read) => {
                        let input = incremental::CommittedDraftInput::new(
                            &self.planning_context(), &next, &result, read,
                        )?;
                        self.persist_committed_draft(*next, prepared, result.clone(),
                            WorkingPublicationInput::Committed(input))?;
                    }
                    None => self.persist(*next, prepared, result.clone())?,
                }
                Ok(result)
            }
        }
    }
}
