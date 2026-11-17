//! One newest original saved revision per controlled FILE. No public command
//! can claim this cause. Expensive material synthesis happens without the live
//! Engine mutex; the sole serialized native decision rechecks the frozen source.
use super::{SelectedWorkingSources, WorkingPublicationInput, business, codec, incremental, require_initial};
use business::automatic::{MESSAGE, Saved, stale};
use crate::container_storage::{PreparedAutomaticFull, PreparedUnbornFull, ReadDraftDigest};
use crate::engine::{MutationPlan, Prepared};
use crate::{Engine, OperationResult, Result, State};
use std::sync::Arc;
use storage_contract::file_draft::{WorkingContentExpectation, WorkingExpectation, WorkingSelection};
use storage_contract::{FileId, OperationId, Principal, Sequence};

mod frozen;
mod fence;
pub(super) use fence::invalidate_changed;
use frozen::Frozen;

pub struct AutomaticFilePreparation {
    frozen: Frozen,
    lease: fence::Lease,
    request: WorkingSelection,
    saved: Saved,
    actor: Principal,
    operation: OperationId,
}

pub struct PreparedAutomaticFile {
    source: AutomaticFilePreparation,
    selection: Arc<SelectedWorkingSources>,
    body: Body,
}

enum Body {
    Born { request: codec::AutomaticRequest, projected: State, versions: Vec<Prepared>,
        content: Option<business::PublishedContent>, read: Option<Arc<ReadDraftDigest>>,
        full: Option<Arc<PreparedAutomaticFull>> },
    First { projected: business::unborn::Projection, versions: Vec<Prepared>,
        content: business::PublishedContent, full: Arc<PreparedUnbornFull> },
}

impl Engine {
    /// Inspect at most 64 rows per scheduler tick. A durable job is not queued;
    /// each capture below rechecks the current controller and original. The
    /// volatile scan position is for fairness only and can restart at the root.
    pub fn automatic_file_candidate(&self, after: Option<FileId>) -> (Option<FileId>, Option<FileId>) {
        if self.config.generation().is_some() || self.current_namespaces.is_some() { return (None, None); }
        let mut it = self.state.file_containers.after(after);
        let mut last = None;
        for _ in 0..64 {
            let Some((&id, record)) = it.next() else { return (None, None); };
            last = Some(id);
            if !record.namespace.parent.is_worktree()
                && record.working.automatic_controller().is_some_and(|c| c.pending_save().is_some())
            { return (last, Some(id)); }
        }
        (last, None)
    }

    pub fn prepare_automatic_file(&self, file: FileId) -> Result<AutomaticFilePreparation> {
        self.require_write()?;
        let context = self.planning_context();
        let row = context.state.file_containers.get(&file).ok_or_else(stale)?;
        let save = row.working.automatic_controller().and_then(|c| c.pending_save()).ok_or_else(stale)?;
        let target = business::ordinary::OrdinaryOccurrence::capture(context.state, file, row)?.target;
        require_initial(&context, &target)?;
        let expected = WorkingExpectation::from_working(&row.working).ok_or_else(stale)?;
        let request = WorkingSelection { target, expected };
        business::selected::require_all(context.state, std::slice::from_ref(&request),
            context.config.max_entries, context.config.max_file_bytes)?;
        let (original, principal) = context.original_receipts.ok_or_else(business::invalid)?
            .file_decision(&context, save.operation)?;
        let saved = Saved::capture(context.state, &request.target, original, principal)?;
        let operation = saved.operation(&request.target)?;
        // This task cannot reinterpret an already occupied operation key as a
        // completed automatic node or mint a second task identity instead.
        if context.state.operations.contains_key(&operation) { return Err(business::invalid()); }
        let lease = fence::Lease::claim(context.config.pool_id.parse()?, file)?;
        Ok(AutomaticFilePreparation { frozen: Frozen::capture(self)?, lease, request, saved,
            actor: principal.clone(), operation })
    }

    pub fn finish_automatic_file(&mut self, work: PreparedAutomaticFile) -> Result<OperationResult> {
        let PreparedAutomaticFile { source, selection, body } = work;
        let AutomaticFilePreparation { request, saved, actor, operation, frozen: _frozen, lease } = source;
        let target = &request.target;
        let publication = codec::unborn_first::FirstPublication::Automatic { saved: saved.clone() };
        let (kind, native) = match &body {
            Body::Born { request, .. } => ("file.draft.commit", codec::request(&codec::Request::Automatic(request.clone()))?),
            Body::First { .. } => (codec::unborn_first::KIND, codec::unborn_first::request(&request, MESSAGE, &publication)?),
        };
        let mut proof = None;
        let plan = self.plan_draft_at(kind, &actor, operation, native, |engine, next, versions, time| {
            let context = engine.planning_context();
            // Existing decided tasks return above before *any* current checks.
            lease.check()?;
            selection.require_context(&context)?;
            require_initial(&context, target)?;
            let (original, principal) = context.original_receipts.ok_or_else(business::invalid)?
                .file_decision(&context, saved.decision.operation)?;
            if principal != &actor { return Err(stale()); }
            saved.require_original(context.state, target, original, principal)?;
            let sequence = Sequence::new(next.sequence);
            match body {
                Body::Born { request, mut projected, versions: prepared, content, read, full } => {
                    request.require_body(context.state)?;
                    if let Some(full) = &full { full.require(&context, target, request.payload)?; }
                    if let Some(read) = &read { read.require_source()?; }
                    let (working, origin) = saved.after(context.state, target, content.as_ref(),
                        actor.as_str(), operation, sequence, time)?;
                    business::apply(&mut projected, target.file, working.clone())?;
                    let out = codec::encode(business::Outcome {
                        request: codec::Request::Automatic(request), working, content, origin,
                    })?;
                    *next = projected;
                    versions.extend(prepared);
                    proof = read;
                    Ok(out)
                }
                Body::First { mut projected, versions: prepared, content, full } => {
                    full.require_context(&context)?;
                    projected.bind_automatic_first(context.state, &saved, &content, &actor, operation, time)?;
                    let origin = business::origin::NodeOrigin::from_content(context.state, target, &content,
                        saved.reason(), operation, sequence, time)?;
                    let out = codec::unborn_first::encode(&request, MESSAGE, &publication, &content, &origin)?;
                    *next = projected.into_state();
                    versions.extend(prepared);
                    Ok(out)
                }
            }
        })?;
        match plan {
            MutationPlan::Existing(receipt) => Ok(receipt),
            MutationPlan::Prepared { next, prepared, result } => {
                if let Some(read) = proof {
                    let input = incremental::CommittedDraftInput::new(&self.planning_context(), &next, &result, read)?;
                    self.persist_committed_draft(*next, prepared, result.clone(), WorkingPublicationInput::Committed(input))?;
                } else { self.persist(*next, prepared, result.clone())?; }
                Ok(result)
            }
        }
    }
}

impl AutomaticFilePreparation {
    pub fn execute(self) -> Result<PreparedAutomaticFile> {
        self.lease.check()?;
        let context = self.frozen.context();
        let request = &self.request;
        let selected = Arc::new(SelectedWorkingSources::capture(&context, std::slice::from_ref(request))?);
        let [source] = selected.sources() else { return Err(business::invalid()); };
        let body = match request.expected.content {
            WorkingContentExpectation::Born { base, payload } => {
                let native = codec::AutomaticRequest { target: request.target.clone(), base, payload,
                    saved: self.saved.clone() };
                let command = native.command();
                let read = source.draft_digest_observed(
                    context.state, context.config.checksum_chunk_bytes, || self.lease.check(),
                )?;
                let mut projected = business::unborn::begin(context.state, Sequence::new(context.next_slot))?;
                let mut versions = Vec::new();
                let (full, content) = if business::draft_matches_base(context.state, &command, read.as_deref())? {
                    (None, None)
                } else {
                    let hash = business::draft_content_hash(context.state, &request.target, read.as_deref())?;
                    self.lease.check()?;
                    let full = PreparedAutomaticFull::prepare(
                        &context, Arc::clone(&selected), hash, || self.lease.check(),
                    )?;
                    let content = business::build_automatic_content(&context, &mut projected, &mut versions,
                        &command, &self.actor, self.operation, Arc::clone(&full), read.as_deref())?;
                    (Some(full), Some(content))
                };
                Body::Born { request: native, projected, versions, content, full, read }
            }
            WorkingContentExpectation::UnbornSaved { .. } => {
                let (primary, copies) = context.new_file_placement_for_file(
                    &request.target.repository.to_string(), Some(&request.target.file.to_string()),
                    request.target.path.as_str())?;
                let disk = context.disk(&primary)?;
                if copies != 1 || disk.level != 1 || disk.upstream_slot_id.is_some()
                    || disk.disk_id.parse::<storage_contract::DiskId>()? != source.record().owner_disk
                { return Err(stale()); }
                business::trace_limit(context.state, &request.target)?;
                self.lease.check()?;
                let full = PreparedUnbornFull::prepare(&context, Arc::clone(&selected), 0)?;
                let (projected, versions, content) = business::unborn::build_first_content(
                    &context, &request.target, request.expected.revision, MESSAGE, &self.actor, self.operation, &full)?;
                Body::First { projected, versions, content, full }
            }
            WorkingContentExpectation::UnbornEmpty => return Err(stale()),
        };
        self.lease.check()?;
        selected.require_context(&context)?;
        Ok(PreparedAutomaticFile { source: self, selection: selected, body })
    }
}
