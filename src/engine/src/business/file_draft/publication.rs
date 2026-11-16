//! One logical working-state plan is consumed by both live and cold admission.
use super::{DraftError as Failure, PublishedContent, content, invalid, selection};
use crate::{Error, OperationResult, Result, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{DraftCommand, DraftPayload, FileWorkingState};
use storage_contract::{Event, FileId, Generation};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Outcome {
    pub(crate) request: DraftCommand,
    pub(crate) working: FileWorkingState,
    pub(crate) content: Option<PublishedContent>,
}

pub(crate) fn plan_change(
    previous: &State,
    request: &DraftCommand,
    payload: Option<DraftPayload>,
) -> Result<FileWorkingState> {
    let selected = selection::resolve(previous, request.target())?;
    let record = previous.file_containers.get(&request.target().file).ok_or_else(invalid)?;
    selection::validate(previous, request.target().file, record)?;
    let change = match request {
        DraftCommand::SetMode { expected, mode, .. } => {
            if payload.is_some() { return Err(invalid()); }
            super::change_mode(&record.working, *expected, *mode)
        }
        DraftCommand::Discard { expected, .. } => {
            if payload.is_some() { return Err(invalid()); }
            super::discard(&record.working, *expected)
        }
        DraftCommand::Write { expected, base, content_hash, logical_size, .. } => {
            let payload = payload.ok_or_else(invalid)?;
            if selected.base != *base {
                return Err(Error::FileDraft(Failure::BaseChanged));
            }
            if payload.content_hash() != *content_hash || payload.logical_size() != *logical_size {
                return Err(invalid());
            }
            super::write(&record.working, *expected, *base, payload)
        }
        DraftCommand::Commit { expected, base, .. } => {
            if payload.is_some() || selected.base != *base {
                return Err(Error::FileDraft(Failure::BaseChanged));
            }
            let commit = super::commit(&record.working, *expected).map_err(Error::FileDraft)?;
            if *commit.base() != *base {
                return Err(Error::FileDraft(Failure::BaseChanged));
            }
            Ok(commit.into_change())
        }
    }.map_err(Error::FileDraft)?;
    Ok(change.into_next())
}

/// Explicit commit compares only the immutable, already admitted draft and
/// its exact base. No history scan and no version allocation are needed.
pub(crate) fn draft_matches_base(previous: &State, request: &DraftCommand) -> Result<bool> {
    let DraftCommand::Commit { target, base, .. } = request else {
        return Err(invalid());
    };
    let working = &previous.file_containers.get(&target.file).ok_or_else(invalid)?.working;
    let (draft_base, payload) = working.draft().ok_or(Error::FileDraft(Failure::DraftRequired))?;
    if draft_base != base {
        return Err(Error::FileDraft(Failure::BaseChanged));
    }
    let version = previous.versions.get(&base.version_id().to_string()).ok_or_else(invalid)?;
    Ok(version.logical_size == payload.logical_size()
        && version.content_hash == payload.content_hash())
}

pub(crate) fn apply(next: &mut State, file: FileId, working: FileWorkingState) -> Result<()> {
    next.file_containers.get_mut(&file).ok_or_else(invalid)?.working = working;
    Ok(())
}

fn preserve(previous: &State, next: &State) -> Result<()> {
    for (file, record) in previous.file_containers.iter() {
        let Some(after) = next.file_containers.get(file) else {
            if record.working.draft().is_some() { return Err(invalid()); }
            continue;
        };
        if record.working != after.working {
            return Err(invalid());
        }
        if let Some((base, _)) = record.working.draft() {
            let target = selection::target(*file, record)?;
            if selection::resolve(next, &target)?.base != *base {
                return Err(Error::FileDraft(Failure::Dirty));
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_working_transition(
    previous: &State,
    next: &State,
    result: &OperationResult,
) -> Result<Option<FileId>> {
    if !crate::persist::file_draft::is_kind(&result.kind) {
        preserve(previous, next)?;
        return Ok(None);
    }
    if previous.operations.contains_key(&result.operation_id)
        || next.operations.get(&result.operation_id) != Some(result)
        || next.sequence <= previous.sequence
    {
        return Err(invalid());
    }
    let decoded = crate::persist::file_draft::decode(previous, result)?;
    let outcome = decoded.outcome();
    let principal = next.operation_principals.get(&result.operation_id).ok_or_else(invalid)?;
    crate::Engine::require_actor(principal)?;
    let payload = match &outcome.request {
        DraftCommand::Write { .. } => {
            let (_, payload) = outcome.working.draft().ok_or_else(invalid)?;
            if crate::persist::file_draft::payload_was_published(previous, payload.payload_id())? {
                return Err(invalid());
            }
            Some(*payload)
        }
        DraftCommand::SetMode { .. } | DraftCommand::Discard { .. }
        | DraftCommand::Commit { .. } => None,
    };
    let working = plan_change(previous, &outcome.request, payload)?;
    if working != outcome.working {
        return Err(invalid());
    }
    let file = outcome.request.target().file;
    let changed = previous.file_containers.get(&file).ok_or_else(invalid)?.working != working;
    let mut expected = previous.clone();
    expected.begin_tracking();
    crate::container_storage::capture_names(&mut expected);
    expected.sequence = next.sequence;
    expected.business_generation = Generation::new(previous.business_generation).next()
        .map_err(|source| Error::FileDraft(Failure::PublicationGeneration(source)))?.get();
    match (&outcome.request, &outcome.content) {
        (DraftCommand::Commit { target, message, .. }, Some(content)) => {
            let selection = selection::resolve(previous, target)?;
            let record = previous.file_containers.get(&file).ok_or_else(invalid)?;
            let (_, payload) = record.working.draft().ok_or_else(invalid)?;
            content::replay(
                previous, &mut expected, target, &selection, *payload, content, message,
                principal, result.operation_id,
            )?;
        }
        (DraftCommand::Commit { .. }, None) => {
            if !draft_matches_base(previous, &outcome.request)? {
                return Err(invalid());
            }
        }
        (DraftCommand::SetMode { .. } | DraftCommand::Write { .. }
        | DraftCommand::Discard { .. }, None) => {}
        _ => return Err(invalid()),
    }
    apply(&mut expected, file, working)?;
    crate::container_storage::capture_changed_names(previous, &mut expected);
    let receipt = decoded.receipt(previous, result.operation_id, next.sequence, principal)?;
    if &receipt != result {
        return Err(invalid());
    }
    expected.operations.insert(result.operation_id, receipt);
    expected.operation_principals.insert(result.operation_id, principal.clone());
    expected.events.push(Event {
        sequence: next.sequence, operation_id: result.operation_id,
        kind: outcome.request.kind().into(), object_id: result.operation_id.to_string(),
    });
    if !crate::publication_shape::same_transition(previous, &expected, next) {
        return Err(invalid());
    }
    Ok(changed.then_some(file))
}
