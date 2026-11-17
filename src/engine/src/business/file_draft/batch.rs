//! One prior State and one accumulating projection for an explicit working set.
//! This is construction, not a native receipt or permission to publish/clear.
use crate::business::file_draft::{
    DraftError as Failure, PublishedContent, content, invalid, selected, selection,
};
use crate::engine::file_draft::SelectedWorkingSources;
use crate::engine::{PlanningContext, Prepared, ReadSource};
use crate::{Error, FileVersion, Result, State};
use std::sync::Arc;
use storage_contract::file_draft::{
    DraftBase, DraftCommand, DraftLocation, FileWorkingState, WorkingContentExpectation,
    WorkingSelection,
};
use storage_contract::{BranchId, FileNodeId, OperationId, Principal, Sequence};

/// Descriptive request input. A live producer also needs the exact complete
/// held selection; this value alone cannot provide a BODY or a working clear.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkingBatchRequest {
    operation: OperationId,
    principal: Principal,
    message: String,
    selected: Vec<WorkingSelection>,
}

impl WorkingBatchRequest {
    pub(crate) fn new(
        operation: OperationId, principal: Principal, message: String,
        selected: Vec<WorkingSelection>,
    ) -> Self {
        Self { operation, principal, message, selected }
    }

    pub(crate) fn operation(&self) -> OperationId { self.operation }
    pub(crate) fn principal(&self) -> &Principal { &self.principal }
    pub(crate) fn message(&self) -> &str { &self.message }
    pub(crate) fn selected(&self) -> &[WorkingSelection] { &self.selected }
}

/// Allocation/output facts only, with no durable encoding registered yet.
/// Replaying this description constructs a projection, never a held producer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkingBatchTrace {
    pub(crate) contents: Vec<Option<PublishedContent>>,
}

pub(crate) enum WorkingBatchResult {
    Retained(FileVersion),
    Published(PublishedContent),
}

pub(crate) struct WorkingBatchItem {
    target: DraftLocation,
    base: DraftBase,
    working_after: FileWorkingState,
    result: WorkingBatchResult,
}

impl WorkingBatchItem {
    pub(crate) fn target(&self) -> &DraftLocation { &self.target }
    pub(crate) fn base(&self) -> DraftBase { self.base }
    pub(crate) fn branch(&self) -> BranchId { self.base.branch_id() }
    pub(crate) fn working_after(&self) -> &FileWorkingState { &self.working_after }
    pub(crate) fn result(&self) -> &WorkingBatchResult { &self.result }

    pub(crate) fn version(&self) -> &FileVersion {
        match &self.result {
            WorkingBatchResult::Retained(version) => version,
            WorkingBatchResult::Published(content) => &content.version,
        }
    }

    pub(crate) fn node(&self) -> Result<FileNodeId> {
        match &self.result {
            WorkingBatchResult::Retained(_) => Ok(self.base.node_id()),
            WorkingBatchResult::Published(content) => {
                Ok(FileNodeId::Version(content.version.version_id.parse()?))
            }
        }
    }
}

/// No operation/event or directory registration has been added. The eventual
/// combined kernel must extend this exact projection and issue one receipt.
pub(crate) struct WorkingBatchProjection {
    state: State,
    items: Vec<WorkingBatchItem>,
    trace: WorkingBatchTrace,
}

impl WorkingBatchProjection {
    pub(crate) fn state(&self) -> &State { &self.state }
    pub(crate) fn items(&self) -> &[WorkingBatchItem] { &self.items }
    pub(crate) fn trace(&self) -> &WorkingBatchTrace { &self.trace }

    /// One native Candidate carries every changed FILE exactly once and no
    /// Retained entry. The cold directory verifier calls this only after its
    /// original admitted pair has reconstructed this complete projection.
    /// Matching evidence is not physical source or publication authority.
    pub(crate) fn require_versions(&self, versions: &[crate::journal::VersionEvidence])
        -> Result<()>
    {
        let mut supplied = versions.iter();
        for item in &self.items {
            if let WorkingBatchResult::Published(content) = item.result() {
                let evidence = supplied.next().ok_or_else(invalid)?;
                if evidence.version != content.version || evidence.placement != content.placement {
                    return Err(invalid());
                }
            }
        }
        if supplied.next().is_some() {
            return Err(invalid());
        }
        Ok(())
    }
}

struct PlannedItem {
    command: DraftCommand,
    working_after: FileWorkingState,
    retained: Option<FileVersion>,
}

fn plan(
    previous: &State, request: &WorkingBatchRequest, sequence: Sequence,
    maximum_entries: usize, maximum_bytes: u64,
) -> Result<Vec<PlannedItem>> {
    if previous.operations.contains_key(&request.operation)
        || sequence.get() <= previous.sequence
    {
        return Err(invalid());
    }
    // One whole-set CAS, including a later stale item, before any version read
    // or allocation. Unborn remains explicitly unsupported by this Born slice.
    let checked = selected::require_all(
        previous, &request.selected, maximum_entries, maximum_bytes,
    )?;
    let mut planned = Vec::with_capacity(checked.len());
    for item in checked {
        let selected = item.request();
        let WorkingContentExpectation::Born { base, .. } = selected.expected.content else {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        };
        let command = DraftCommand::Commit {
            target: selected.target.clone(), expected: selected.expected.revision,
            base, message: request.message.clone(),
        };
        let working_after = super::commit(&item.record().working, selected.expected.revision)
            .map_err(Error::FileDraft)?.into_change().into_next();
        let retained = if super::draft_matches_base(previous, &command)? {
            Some(previous.versions.get(&base.version_id().to_string())
                .cloned().ok_or_else(invalid)?)
        } else {
            None
        };
        planned.push(PlannedItem { command, working_after, retained });
    }
    Ok(planned)
}

fn project(
    previous: &State, request: &WorkingBatchRequest, sequence: Sequence,
    planned: Vec<PlannedItem>,
    mut build: impl FnMut(usize, &mut State, &DraftCommand) -> Result<PublishedContent>,
) -> Result<WorkingBatchProjection> {
    // Shared initialization is a pure clone/generation/name projection. It is
    // not the Unborn issuer and grants neither its BODY nor its native mode.
    let mut next = super::unborn::begin(previous, sequence)?;
    let mut items = Vec::with_capacity(planned.len());
    let mut contents = Vec::with_capacity(planned.len());
    for (index, item) in planned.into_iter().enumerate() {
        let DraftCommand::Commit { target, base, .. } = &item.command else {
            return Err(invalid());
        };
        let result = match item.retained {
            Some(version) => {
                contents.push(None);
                WorkingBatchResult::Retained(version)
            }
            None => {
                let content = build(index, &mut next, &item.command)?;
                contents.push(Some(content.clone()));
                WorkingBatchResult::Published(content)
            }
        };
        // This mutates only the local accumulated projection. Nothing escapes
        // if a later source/constructor fails, and no per-file receipt exists.
        super::apply(&mut next, target.file, item.working_after.clone())?;
        items.push(WorkingBatchItem {
            target: target.clone(), base: *base, working_after: item.working_after, result,
        });
    }
    crate::container_storage::capture_changed_names(previous, &mut next);
    // All selected changes used one operation; none is independently installed.
    // The complete owner table may change only these working rows. No other
    // dirty body, namespace owner or physical identity can be normalized away.
    let mut containers = previous.file_containers.clone();
    for item in &items {
        containers.get_mut(&item.target.file).ok_or_else(invalid)?.working =
            item.working_after.clone();
        selection::validate(&next, item.target.file,
            next.file_containers.get(&item.target.file).ok_or_else(invalid)?)?;
    }
    if next.file_containers != containers
        || next.namespace_directories != previous.namespace_directories
    {
        return Err(invalid());
    }
    if next.operations != previous.operations || next.events != previous.events
        || next.operation_principals != previous.operation_principals
        || next.operations.contains_key(&request.operation)
    {
        return Err(invalid());
    }
    Ok(WorkingBatchProjection { state: next, items, trace: WorkingBatchTrace { contents } })
}

pub(crate) fn build(
    context: &PlanningContext<'_>, selection: &Arc<SelectedWorkingSources>,
    request: &WorkingBatchRequest,
) -> Result<(WorkingBatchProjection, Vec<Prepared>)> {
    selection.require_context(context)?;
    if request.selected.len() != selection.sources().len()
        || request.selected.iter().zip(selection.sources())
            .any(|(request, source)| request != source.selection())
    {
        return Err(Error::FileDraft(Failure::SourceChanged));
    }
    let sequence = Sequence::new(context.next_slot);
    let planned = plan(
        context.state, request, sequence, context.config.max_entries, context.config.max_file_bytes,
    )?;
    // Resolve every changed item's placement policy before constructing any
    // Version. All payloads then come from the same complete held selection.
    let mut policies = Vec::with_capacity(planned.len());
    for item in &planned {
        policies.push(if item.retained.is_none() {
            Some(content::plan(context, &item.command)?)
        } else {
            None
        });
    }
    let mut prepared = Vec::new();
    let projection = project(context.state, request, sequence, planned, |index, next, command| {
        selection.require_context(context)?;
        let policy = policies[index].as_ref().ok_or_else(invalid)?;
        let source = ReadSource::selected_working(Arc::clone(selection), index)?;
        let content = content::build_planned(
            context, next, &mut prepared, command, request.principal.as_str(),
            request.operation, policy, &source,
        )?;
        selection.require_context(context)?;
        Ok(content)
    })?;
    Ok((projection, prepared))
}

/// Deterministic logical reconstruction. The cold native caller must still
/// provide its original admitted pair and genuine held material obligations.
pub(crate) fn replay(
    previous: &State, request: &WorkingBatchRequest, sequence: Sequence,
    trace: &WorkingBatchTrace, maximum_entries: usize, maximum_bytes: u64,
) -> Result<WorkingBatchProjection> {
    let planned = plan(previous, request, sequence, maximum_entries, maximum_bytes)?;
    if trace.contents.len() != planned.len()
        || planned.iter().zip(&trace.contents)
            .any(|(item, content)| item.retained.is_some() == content.is_some())
    {
        return Err(invalid());
    }
    project(previous, request, sequence, planned, |index, expected, command| {
        let DraftCommand::Commit { target, message, .. } = command else {
            return Err(invalid());
        };
        let content = trace.contents[index].as_ref().ok_or_else(invalid)?;
        let selection = selection::resolve(previous, target)?;
        let record = previous.file_containers.get(&target.file).ok_or_else(invalid)?;
        let (_, payload) = record.working.draft().ok_or_else(invalid)?;
        content::replay(
            previous, expected, target, &selection, *payload, content, message,
            request.principal.as_str(), request.operation,
        )?;
        Ok(content.clone())
    })
}
