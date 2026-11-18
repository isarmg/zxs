//! Mandatory attribution on a genuine Workspace decision. A Workspace commit
//! kind, a printable path or a visible overlay never issues a FILE origin.
use crate::{
    CommitRequest, Error, FileVersion, OperationResult, OriginalReceiptError, Placement, Result,
    State, Workspace, engine::{PlanningContext, Prepared}, journal::Candidate,
};
use data_serialization::{Deserialize, Serialize};
use storage_contract::{
    DiskId, EntryId, FileNodeId, OperationId, Principal, Sequence, Timestamp,
    file_draft::DraftLocation,
};

mod input;
mod subject;
use crate::snapshot_worktree::ReceiptIndex;
use input::Source;
use subject::Subject;

const FIELD: &str = "workspace_file_origin";
const KIND: &str = "workspace.commit";

fn invalid() -> Error { Error::OriginalReceipt(OriginalReceiptError::Candidate) }

pub(crate) fn prior_error(error: crate::snapshot_worktree::RegistrationError) -> Error {
    Error::OriginalReceipt(OriginalReceiptError::WorkspaceReservation(Box::new(error.into())))
}

/// Distinct from B's on-disk origin envelope. E's reason is fixed by this
/// authenticated explicit operation; this read value is not serializable.
pub(crate) struct VerifiedWorkspaceNode {
    pub(crate) target: DraftLocation,
    pub(crate) owner: DiskId,
    pub(crate) ancestors: Vec<EntryId>,
    pub(crate) node: FileNodeId,
    pub(crate) operation: OperationId,
    pub(crate) sequence: Sequence,
    pub(crate) published_at: Timestamp,
}

#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Attribution {
    NoSuchCreation { source: Option<Source> },
    ManualFile { source: Source, binding: subject::Binding },
}
/// Counting the compact integer-only JSON is exact: sorting object keys for
/// canonical encoding does not change length. Bound the output before making
/// a second Value or encoded byte buffer for an attacker-sized sealed request.
fn bounded(value: &(impl Serialize + ?Sized), maximum: usize) -> Result<()> {
    struct Count { left: usize, exceeded: bool }
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let Some(left) = self.left.checked_sub(bytes.len()) else {
                self.exceeded = true;
                return Err(std::io::Error::other("Workspace original exceeds metadata byte budget"));
            };
            self.left = left;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut count = Count { left: maximum, exceeded: false };
    match json_codec::to_writer(&mut count, value) {
        Ok(()) => Ok(()),
        Err(_) if count.exceeded => Err(Error::new(storage_contract::ErrorCode::ResourceLimit,
            "Workspace original exceeds metadata byte budget")),
        Err(e) => Err(e.into()),
    }
}

fn as_value(record: &Attribution) -> Result<json_codec::Value> {
    bounded(record, crate::disk::MAX_METADATA)?;
    Ok(json_codec::to_value(record)?)
}
fn read(result: &OperationResult) -> Result<Attribution> {
    let value = result.result.get(FIELD).ok_or_else(invalid)?;
    bounded(value, crate::disk::MAX_METADATA)?;
    let record = Attribution::deserialize(value)?;
    if json_codec::to_value(&record)? != *value { return Err(invalid()); }
    Ok(record)
}

fn relevant(result: &OperationResult) -> Result<bool> {
    if result.kind == KIND { return Ok(true); }
    if result.result.get(FIELD).is_some() { return Err(invalid()); }
    Ok(false)
}

/// Absence of a Prepared/Candidate item is not evidence that an existing FILE
/// did not acquire a version. Workspace only appends versions and placements.
fn material<'a>(
    previous: &State, next: &State, result: &OperationResult,
    actual: impl ExactSizeIterator<Item = (&'a FileVersion, &'a Placement)>,
) -> Result<Option<(&'a FileVersion, &'a Placement)>> {
    let count = actual.len();
    if previous.versions.len().checked_add(count) != Some(next.versions.len())
        || previous.placements.len().checked_add(count) != Some(next.placements.len())
    { return Err(invalid()); }
    let mut ids = std::collections::BTreeSet::new();
    let mut sole = None;
    for (version, placement) in actual {
        if placement.version_id != version.version_id || version.version_sequence != result.sequence
            || !ids.insert(version.version_id.as_str()) || previous.versions.contains_key(&version.version_id)
            || previous.placements.contains_key(&version.version_id)
            || next.versions.get(&version.version_id) != Some(version)
            || next.placements.get(&version.version_id) != Some(placement)
        { return Err(invalid()); }
        if count == 1 { sole = Some((version, placement)); }
    }
    Ok(sole)
}

pub(crate) fn attach(
    context: &PlanningContext<'_>, workspace: &Workspace, request: &CommitRequest,
    wire: Option<&crate::wire::WireCommit>, next: &mut State, result: &mut OperationResult,
    prepared: &[Prepared],
) -> Result<()> {
    if result.kind != KIND || result.result.get(FIELD).is_some()
        || next.operations.get(&result.operation_id) != Some(result)
    { return Err(invalid()); }
    let initial = context.config.generation().is_none() && context.current_namespaces.is_none();
    let sole = material(context.state, next, result, prepared.iter().map(|p| (&p.version, &p.placement)))?;
    let subject = Subject::observe(initial, context.state, next, result, sole)?;
    let source = subject.as_ref().map(|subject| {
        Source::capture(workspace, request, wire, &subject.binding)
    }).transpose()?;
    let actor = Principal::new(request.principal.as_str().into())
        .map_err(|e| Error::OriginalReceipt(OriginalReceiptError::Principal(e)))?;
    let record = derive(context.state, next, subject.as_ref(), source, result, &actor,
        || context.snapshot_receipts().map_err(prior_error))?;
    result.result[FIELD] = as_value(&record)?;
    bounded(result, crate::disk::MAX_METADATA)?;
    next.operations.insert(result.operation_id, result.clone());
    Ok(())
}

fn derive<'a>(
    previous: &'a State, next: &State, subject: Option<&Subject>, source: Option<Source>,
    result: &OperationResult, actor: &Principal,
    prior: impl FnOnce() -> Result<ReceiptIndex<'a>>,
) -> Result<Attribution> {
    match (subject, source) {
        (None, None) => Ok(Attribution::NoSuchCreation { source: None }),
        (Some(subject), Some(source)) => {
            source.require_original(result, actor.as_str(), &subject.binding)?;
            if source.proves_created(&subject.binding)? {
                source.require_base(previous, &subject.binding)?;
                source.require_message(next, result)?;
                subject.binding.require_retained(next, result, actor.as_str())?;
                if prior()?.require_workspace_child(previous, next, result, actor)? {
                    return Ok(Attribution::NoSuchCreation { source: Some(source) });
                }
                Ok(Attribution::ManualFile { source, binding: subject.binding.clone() })
            } else {
                Ok(Attribution::NoSuchCreation { source: Some(source) })
            }
        }
        _ => Err(invalid()),
    }
}

fn transition<'a>(
    initial: bool, previous: &'a State, next: &State, result: &OperationResult, actor: &Principal,
    sole: Option<(&FileVersion, &Placement)>, prior: impl FnOnce() -> Result<ReceiptIndex<'a>>,
) -> Result<Option<VerifiedWorkspaceNode>> {
    if previous.operations.contains_key(&result.operation_id) || next.sequence != result.sequence
        || next.operations.get(&result.operation_id) != Some(result)
        || next.operation_principals.get(&result.operation_id).map(String::as_str) != Some(actor.as_str())
    { return Err(invalid()); }
    let subject = Subject::observe(initial, previous, next, result, sole)?;
    let stored = read(result)?;
    let source = match stored {
        Attribution::NoSuchCreation { source } => source,
        Attribution::ManualFile { source, .. } => Some(source),
    };
    let expected = derive(previous, next, subject.as_ref(), source, result, actor, prior)?;
    if result.result.get(FIELD) != Some(&as_value(&expected)?) { return Err(invalid()); }
    match expected {
        Attribution::NoSuchCreation { .. } => Ok(None),
        Attribution::ManualFile { binding, .. } => Ok(Some(binding.node())),
    }
}

pub(crate) fn validate_prepared<'a>(
    initial: bool, previous: &'a State, next: &State, result: &OperationResult,
    actor: &Principal, prepared: &[Prepared], prior: impl FnOnce() -> Result<ReceiptIndex<'a>>,
) -> Result<Option<VerifiedWorkspaceNode>> {
    if !relevant(result)? { return Ok(None); }
    let sole = material(previous, next, result, prepared.iter().map(|p| (&p.version, &p.placement)))?;
    transition(initial, previous, next, result, actor, sole, prior)
}

pub(crate) fn validate<'a>(
    candidate: &Candidate, previous: &'a State, next: &State, result: &OperationResult, actor: &Principal,
    prior: impl FnOnce() -> Result<ReceiptIndex<'a>>,
) -> Result<Option<VerifiedWorkspaceNode>> {
    if !relevant(result)? { return Ok(None); }
    if candidate.operation_id != result.operation_id || candidate.sequence != result.sequence
        || candidate.request_digest != result.request_digest { return Err(invalid()); }
    let sole = material(previous, next, result, candidate.versions.iter().map(|v| (&v.version, &v.placement)))?;
    transition(candidate.generation().is_none(), previous, next, result, actor, sole, prior)
}

/// Only for the original retained snapshot after admission. Does not certify
/// an unseen transition, look up a new current name, consult a live Workspace,
/// upgrade a visible recovery result, or supply automatic-save statistics.
pub(crate) fn validate_retained(
    state: &State, result: &OperationResult, actor: &Principal,
) -> Result<Option<VerifiedWorkspaceNode>> {
    if !relevant(result)? { return Ok(None); }
    if state.sequence < result.sequence
        || state.operation_principals.get(&result.operation_id).map(String::as_str) != Some(actor.as_str())
    { return Err(invalid()); }
    let stored = read(result)?;
    match stored {
        Attribution::NoSuchCreation { source } => {
            if let Some(source) = source { source.require_digest(result, actor.as_str())?; }
            Ok(None)
        }
        Attribution::ManualFile { source, binding } => {
            binding.require_retained(state, result, actor.as_str())?;
            source.require_base(state, &binding)?;
            source.require_message(state, result)?;
            source.require_original(result, actor.as_str(), &binding)?;
            if !source.proves_created(&binding)? { return Err(invalid()); }
            Ok(Some(binding.node()))
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/workspace_file_origin_tests.rs"]
mod tests;
