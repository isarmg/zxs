//! One prior State and one accumulating projection for an explicit working set.
//! This is construction, not a native receipt or permission to publish/clear.
use crate::business::file_draft::{
    DraftError as Failure, PublishedContent, content, invalid, selected, selection,
};
use crate::engine::file_draft::SelectedWorkingSources;
use crate::engine::{PlanningContext, Prepared, ReadSource};
use crate::{Error, FileVersion, Result, State, ValidatedConfig};
use crate::container_storage::{MeasuredUnbornFull, ReadWorkingDigest, ReadDraftDigest};
use crate::business::file_draft::unborn::first::{ConstructionMaterial, FirstContentConstruction, FirstCommit};
use crate::business::namespace::working::WorkingNamespace;
use std::sync::Arc;
use storage_contract::file_draft::{
    DraftBase, DraftCommand, DraftLocation, FileWorkingState, WorkingContentExpectation,
    WorkingRevision, WorkingSelection,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkingBatchOrigin {
    Born(DraftBase),
    First { branch: BranchId },
}

pub(crate) struct WorkingBatchItem {
    target: DraftLocation,
    origin: WorkingBatchOrigin,
    working_after: FileWorkingState,
    result: WorkingBatchResult,
}

impl WorkingBatchItem {
    pub(crate) fn target(&self) -> &DraftLocation { &self.target }
    pub(crate) fn origin(&self) -> WorkingBatchOrigin { self.origin }
    pub(crate) fn branch(&self) -> BranchId {
        match self.origin {
            WorkingBatchOrigin::Born(base) => base.branch_id(),
            WorkingBatchOrigin::First { branch } => branch,
        }
    }
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
            WorkingBatchResult::Retained(_) => match self.origin {
                WorkingBatchOrigin::Born(base) => Ok(base.node_id()),
                WorkingBatchOrigin::First { .. } => Err(invalid()),
            },
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

enum PlannedItem {
    Born {
        command: DraftCommand,
        working_after: FileWorkingState,
        retained: Option<FileVersion>,
    },
    First {
        target: DraftLocation,
        expected: WorkingRevision,
        working_after: FileWorkingState,
    },
}

impl PlannedItem {
    fn target(&self) -> &DraftLocation {
        match self {
            Self::Born { command, .. } => command.target(),
            Self::First { target, .. } => target,
        }
    }

    fn working_after(&self) -> &FileWorkingState {
        match self {
            Self::Born { working_after, .. } | Self::First { working_after, .. } => working_after,
        }
    }

    fn retained(&self) -> Option<&FileVersion> {
        match self {
            Self::Born { retained, .. } => retained.as_ref(),
            Self::First { .. } => None,
        }
    }
}

fn plan(
    previous: &State, request: &WorkingBatchRequest, sequence: Sequence,
    maximum_entries: usize, maximum_bytes: u64,
    reads: &[Option<Arc<ReadDraftDigest>>],
) -> Result<Vec<PlannedItem>> {
    if previous.operations.contains_key(&request.operation)
        || sequence.get() <= previous.sequence
    {
        return Err(invalid());
    }
    // Every Born and Unborn expectation is checked before materialization or
    // Version allocation. A later stale item cannot follow an earlier clear.
    let checked = selected::require_all(
        previous, &request.selected, maximum_entries, maximum_bytes,
    )?;
    if reads.len() != checked.len() { return Err(invalid()); }
    let mut planned = Vec::with_capacity(checked.len());
    for (index, item) in checked.into_iter().enumerate() {
        if item.record().working.incremental().is_some() != reads[index].is_some() {
            return Err(invalid());
        }
        let selected = item.request();
        let planned_item = match selected.expected.content {
            WorkingContentExpectation::Born { base, .. } => {
                let command = DraftCommand::Commit {
                    target: selected.target.clone(), expected: selected.expected.revision,
                    base, message: request.message.clone(),
                };
                let working_after = super::commit(
                    &item.record().working, selected.expected.revision,
                ).map_err(Error::FileDraft)?.into_change().into_next();
                let retained = if super::draft_matches_base(previous, &command, reads[index].as_deref())? {
                    Some(previous.versions.get(&base.version_id().to_string())
                        .cloned().ok_or_else(invalid)?)
                } else {
                    None
                };
                PlannedItem::Born { command, working_after, retained }
            }
            WorkingContentExpectation::UnbornEmpty
            | WorkingContentExpectation::UnbornSaved { .. } => {
                let working = item.record().working.unborn().ok_or_else(invalid)?;
                let working_after = working.after_first_commit()
                    .map_err(|source| Error::FileDraft(Failure::RevisionExhausted(source)))?;
                PlannedItem::First {
                    target: selected.target.clone(), expected: selected.expected.revision,
                    working_after,
                }
            }
        };
        planned.push(planned_item);
    }
    Ok(planned)
}

fn project(
    previous: &State, request: &WorkingBatchRequest, sequence: Sequence,
    planned: &[PlannedItem],
    mut build: impl FnMut(usize, &mut State, &PlannedItem) -> Result<PublishedContent>,
) -> Result<WorkingBatchProjection> {
    let mut next = super::unborn::begin(previous, sequence)?;
    let mut items = Vec::with_capacity(planned.len());
    let mut contents = Vec::with_capacity(planned.len());
    for (index, item) in planned.iter().enumerate() {
        let target = item.target();
        let result = match item.retained() {
            Some(version) => {
                contents.push(None);
                WorkingBatchResult::Retained(version.clone())
            }
            None => {
                let content = build(index, &mut next, item)?;
                contents.push(Some(content.clone()));
                WorkingBatchResult::Published(content)
            }
        };
        let origin = match item {
            PlannedItem::Born { command: DraftCommand::Commit { base, .. }, .. } => {
                WorkingBatchOrigin::Born(*base)
            }
            PlannedItem::Born { .. } => return Err(invalid()),
            PlannedItem::First { .. } => WorkingBatchOrigin::First {
                branch: selection::resolve(&next, target)?.base.branch_id(),
            },
        };
        // The first append and Born publisher share this accumulated next.
        // No per-file receipt or intermediate State is installed.
        super::apply(&mut next, target.file, item.working_after().clone())?;
        items.push(WorkingBatchItem {
            target: target.clone(), origin, working_after: item.working_after().clone(), result,
        });
    }
    crate::container_storage::capture_changed_names(previous, &mut next);
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

enum Construction<'a> {
    Retained,
    Born(content::PlannedContent),
    First(FirstContentConstruction<'a>),
}

pub(crate) fn build(
    context: &PlanningContext<'_>, selection: &Arc<SelectedWorkingSources>,
    request: &WorkingBatchRequest,
) -> Result<(WorkingBatchProjection, Vec<Prepared>, Vec<Option<Arc<MeasuredUnbornFull>>>,
    Vec<Option<Arc<ReadDraftDigest>>>)> {
    selection.require_context(context)?;
    if request.selected.len() != selection.sources().len()
        || request.selected.iter().zip(selection.sources())
            .any(|(request, source)| request != source.selection())
    {
        return Err(Error::FileDraft(Failure::SourceChanged));
    }
    let sequence = Sequence::new(context.next_slot);
    let reads = selection.sources().iter().map(|source| {
        source.draft_digest(context.state, context.config.checksum_chunk_bytes)
    }).collect::<Result<Vec<_>>>()?;
    let planned = plan(
        context.state, request, sequence, context.config.max_entries, context.config.max_file_bytes,
        &reads,
    )?;
    let materials = planned.iter().enumerate().map(|(index, item)| match item {
        PlannedItem::Born { .. } => Ok(None),
        PlannedItem::First { .. } => {
            MeasuredUnbornFull::capture(context, selection, index).map(Some)
        }
    }).collect::<Result<Vec<_>>>()?;
    // All policies and held source checks complete before constructing a Version.
    let policies = planned.iter().zip(&materials).map(|(item, material)| match item {
        PlannedItem::Born { retained: Some(_), .. } => Ok(Construction::Retained),
        PlannedItem::Born { command, retained: None, .. } => {
            content::plan(context, command).map(Construction::Born)
        }
        PlannedItem::First { target, expected, .. } => {
            FirstContentConstruction::prepare(
                context, target, *expected, &request.message, &request.principal,
                request.operation, ConstructionMaterial::Measured(material.as_deref().ok_or_else(invalid)?),
            ).map(Construction::First)
        }
    }).collect::<Result<Vec<_>>>()?;
    let mut prepared = Vec::new();
    // The plans borrow the frozen request and materials, not the accumulated State.
    let projection = project(context.state, request, sequence, &planned, |index, next, item| {
        selection.require_context(context)?;
        let content = match (&policies[index], item) {
            (Construction::Born(policy), PlannedItem::Born { command, .. }) => {
                let source = ReadSource::selected_working(Arc::clone(selection), index)?;
                content::build_planned(
                    context, next, &mut prepared, command, request.principal.as_str(),
                    request.operation, policy, &source, reads[index].as_deref(),
                )?
            }
            (Construction::First(construction), PlannedItem::First { .. }) => {
                construction.append(context, next, &mut prepared)?
            }
            _ => return Err(invalid()),
        };
        selection.require_context(context)?;
        Ok(content)
    })?;
    drop(policies);
    Ok((projection, prepared, materials, reads))
}

/// Complete deterministic replay still needs the actual predecessor BODY for
/// each first commit. A serialized full hash or sparse commitment is not that proof.
pub(crate) fn replay(
    previous: &State, config: &ValidatedConfig, request: &WorkingBatchRequest,
    sequence: Sequence, trace: &WorkingBatchTrace,
    first_reads: &[Option<ReadWorkingDigest<'_>>],
    reads: &[Option<Arc<ReadDraftDigest>>],
) -> Result<WorkingBatchProjection> {
    let planned = plan(previous, request, sequence, config.max_entries, config.max_file_bytes, reads)?;
    if trace.contents.len() != planned.len()
        || first_reads.len() != planned.len()
        || planned.iter().zip(&trace.contents)
            .any(|(item, content)| item.retained().is_some() == content.is_some())
        || planned.iter().zip(first_reads)
            .any(|(item, read)| matches!(item, PlannedItem::First { .. }) != read.is_some())
    {
        return Err(invalid());
    }
    project(previous, request, sequence, &planned, |index, expected, item| {
        let facts = trace.contents[index].as_ref().ok_or_else(invalid)?;
        match item {
            PlannedItem::Born { command: DraftCommand::Commit { target, message, .. }, .. } => {
                let selection = selection::resolve(previous, target)?;
                let record = previous.file_containers.get(&target.file).ok_or_else(invalid)?;
                let (_, payload) = record.working.draft().ok_or_else(invalid)?;
                content::replay(
                    previous, expected, target, &selection, *payload, facts, message,
                    request.principal.as_str(), request.operation, reads[index].as_deref(),
                )?;
            }
            PlannedItem::Born { .. } => return Err(invalid()),
            PlannedItem::First { target, expected: revision, .. } => {
                let namespace = WorkingNamespace::from_main_state(
                    previous, target.repository, config.max_entries,
                )?;
                super::unborn::first::project(
                    previous, config, &namespace, expected, request.operation,
                    &FirstCommit {
                        target, expected: *revision, message: &request.message,
                        principal: &request.principal, content: facts,
                        read: first_reads[index].as_ref().ok_or_else(invalid)?,
                    },
                )?;
            }
        }
        Ok(facts.clone())
    })
}
