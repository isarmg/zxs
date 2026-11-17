//! One logical working-state plan is consumed by both live and cold admission.
use super::{DraftError as Failure, PublishedContent, content, invalid, selection};
use crate::{Error, OperationResult, Result, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{AutomaticBody, DraftCommand, DraftPayload, FileWorkingState, SaveController};
use storage_contract::{Event, FileId, Generation};
use crate::persist::file_draft::{Request, RangeSaveRequest};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Outcome {
    pub(crate) request: Request,
    pub(crate) working: FileWorkingState,
    pub(crate) content: Option<PublishedContent>,
}

/// From the single real new-decision clock, or the original receipt on replay.
#[derive(Clone, Copy)]
pub(crate) struct SaveStamp {
    operation: storage_contract::OperationId,
    sequence: storage_contract::Sequence,
    time: storage_contract::Timestamp,
}
impl SaveStamp {
    pub(crate) fn new(operation: storage_contract::OperationId, sequence: u64, time: storage_contract::Timestamp) -> Self {
        Self { operation, sequence: storage_contract::Sequence::new(sequence), time }
    }
}

fn saved(
    current: &FileWorkingState, expected: storage_contract::file_draft::WorkingRevision,
    controller: SaveController, base: storage_contract::file_draft::DraftBase, payload: DraftPayload,
    manifest: Option<&storage_contract::file_draft::IncrementalManifest>, stamp: SaveStamp,
) -> Result<FileWorkingState> {
    super::require_revision(current, expected).map_err(Error::FileDraft)?;
    if current.save_controller() != controller || current.unborn().is_some() {
        return Err(Error::FileDraft(Failure::SourceChanged));
    }
    if current.draft().is_some_and(|(original, _)| *original != base) {
        return Err(Error::FileDraft(Failure::BaseChanged));
    }
    let revision = super::next_revision(current).map_err(Error::FileDraft)?;
    if let FileWorkingState::Automatic { controller, .. } = current {
        let controller = controller.saved(revision, stamp.operation, stamp.sequence, stamp.time)
            .map_err(|e| Error::FileDraft(Failure::Controller(e)))?;
        let body = match manifest {
            Some(manifest) => AutomaticBody::Incremental { base, payload, manifest: manifest.clone() },
            None => AutomaticBody::Full { base, payload },
        };
        return Ok(FileWorkingState::Automatic { revision, controller, body });
    }
    Ok(match manifest {
        Some(manifest) => FileWorkingState::ManualIncremental { revision, base, payload, manifest: manifest.clone() },
        None => super::write(current, expected, base, payload).map_err(Error::FileDraft)?.into_next(),
    })
}

pub(crate) fn plan_change(
    previous: &State,
    request: &DraftCommand,
    payload: Option<DraftPayload>,
    stamp: SaveStamp,
) -> Result<FileWorkingState> {
    let target = request.target();
    let record = previous.file_containers.get(&target.file).ok_or_else(invalid)?;
    if let Some(working) = record.working.unborn() {
        if payload.is_some() { return Err(invalid()); }
        let expected = match request {
            DraftCommand::SetMode { expected, .. } | DraftCommand::ResumeAutomatic { expected, .. } => *expected,
            _ => return Err(Error::FileDraft(Failure::UnbornRequired)),
        };
        let namespace = crate::business::namespace::working::WorkingNamespace::from_main_state(
            previous, target.repository, crate::working_tree::snapshot::MAX_ENTRIES,
        )?;
        super::unborn::selected(&namespace, target, expected)?;
        let after = match request {
            DraftCommand::SetMode { mode, .. } => {
                if working.mode() == storage_contract::file_draft::CommitMode::Manual
                    && mode.mode() == storage_contract::file_draft::CommitMode::Automatic
                    && !matches!(working.body(), storage_contract::file_draft::UnbornBody::Empty {})
                { return Err(Error::FileDraft(Failure::Dirty)); }
                working.change_mode(mode.mode(), stamp.operation)
                    .map_err(|e| Error::FileDraft(Failure::RevisionExhausted(e)))?
            }
            DraftCommand::ResumeAutomatic { epoch, .. } =>
                working.resume(*epoch, stamp.operation).map_err(super::unborn::save_error)?,
            _ => return Err(invalid()),
        };
        return Ok(FileWorkingState::Unborn { working: after });
    }
    let selected = selection::resolve(previous, target)?;
    selection::validate(previous, request.target().file, record)?;
    let change = match request {
        DraftCommand::SetMode { expected, mode, .. } => {
            if payload.is_some() { return Err(invalid()); }
            super::change_mode(&record.working, *expected, mode.mode())
        }
        DraftCommand::ResumeAutomatic { expected, epoch, .. } => {
            if payload.is_some() { return Err(invalid()); }
            super::require_revision(&record.working, *expected).map_err(Error::FileDraft)?;
            let FileWorkingState::Automatic { controller, body, .. } = &record.working else {
                return Err(Error::FileDraft(Failure::AutomaticRequired));
            };
            let revision = super::next_revision(&record.working).map_err(Error::FileDraft)?;
            let controller = controller.resumed(*epoch, revision)
                .map_err(|e| Error::FileDraft(Failure::Controller(e)))?;
            return Ok(FileWorkingState::Automatic { revision, controller, body: body.clone() });
        }
        DraftCommand::Discard { expected, .. } => {
            if payload.is_some() { return Err(invalid()); }
            super::discard(&record.working, *expected)
        }
        DraftCommand::Write { expected, controller, base, content_hash, logical_size, .. } => {
            let payload = payload.ok_or_else(invalid)?;
            if selected.base != *base {
                return Err(Error::FileDraft(Failure::BaseChanged));
            }
            if payload.content_hash() != Some(*content_hash) || payload.logical_size() != *logical_size {
                return Err(invalid());
            }
            return saved(&record.working, *expected, *controller, *base, payload, None, stamp);
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

pub(crate) fn plan_incremental(
    previous: &State, request: &RangeSaveRequest, payload: DraftPayload,
    manifest: &storage_contract::file_draft::IncrementalManifest,
    stamp: SaveStamp,
) -> Result<FileWorkingState> {
    let target = &request.target;
    let selected = selection::resolve(previous, target)?;
    let record = previous.file_containers.get(&target.file).ok_or_else(invalid)?;
    selection::validate(previous, target.file, record)?;
    let current = &record.working;
    if selected.base != request.base { return Err(Error::FileDraft(Failure::BaseChanged)); }
    if !matches!(current, FileWorkingState::ManualClean { .. }
        | FileWorkingState::ManualIncremental { .. }
        | FileWorkingState::Automatic { body: AutomaticBody::Clean {} | AutomaticBody::Incremental { .. }, .. })
    {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let version = previous.versions.get(&request.base.version_id().to_string())
        .ok_or_else(invalid)?;
    manifest.require_previous(current.incremental(), version.logical_size, u64::MAX)
        .map_err(|source| Error::FileDraft(Failure::WorkingManifest(source)))?;
    if manifest.material().file() != target.file
        || manifest.material().operation() != stamp.operation
        || manifest.material().logical_size() != payload.logical_size()
        || request.logical_size != payload.logical_size()
        || payload.manifest_commitment() != Some(manifest.material().commitment())
        || request.manifest_commitment != manifest.material().commitment()
    {
        return Err(invalid());
    }
    saved(current, request.expected, request.controller, request.base, payload, Some(manifest), stamp)
}

/// A range identity alone never supplies a ContentHash. The explicit commit's
/// actual read proof is retained through the live/cold publication gate.
pub(crate) fn draft_content_hash(
    previous: &State, target: &storage_contract::file_draft::DraftLocation,
    read: Option<&crate::container_storage::ReadDraftDigest>,
) -> Result<storage_contract::ContentHash> {
    let record = previous.file_containers.get(&target.file).ok_or_else(invalid)?;
    let (_, payload) = record.working.draft().ok_or(Error::FileDraft(Failure::DraftRequired))?;
    match (payload.content_hash(), read) {
        (Some(hash), None) => Ok(hash),
        (None, Some(read)) => {
            read.require_record(target.file, record)?;
            Ok(read.content_hash())
        }
        (Some(_), Some(_)) | (None, None) => Err(invalid()),
    }
}

pub(crate) fn draft_matches_base(
    previous: &State, request: &DraftCommand,
    read: Option<&crate::container_storage::ReadDraftDigest>,
) -> Result<bool> {
    let DraftCommand::Commit { target, base, .. } = request else { return Err(invalid()); };
    let working = &previous.file_containers.get(&target.file).ok_or_else(invalid)?.working;
    let (draft_base, payload) = working.draft().ok_or(Error::FileDraft(Failure::DraftRequired))?;
    if draft_base != base { return Err(Error::FileDraft(Failure::BaseChanged)); }
    let hash = draft_content_hash(previous, target, read)?;
    let version = previous.versions.get(&base.version_id().to_string()).ok_or_else(invalid)?;
    Ok(version.logical_size == payload.logical_size() && version.content_hash == hash)
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
            let target = selection::target(previous, *file, record)?;
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
    read: Option<&crate::container_storage::ReadDraftDigest>,
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
        Request::Command(DraftCommand::Write { .. }) | Request::Ranges(_) => {
            let (_, payload) = outcome.working.draft().ok_or_else(invalid)?;
            if crate::persist::file_draft::payload_was_published(previous, payload.payload_id())? {
                return Err(invalid());
            }
            Some(*payload)
        }
        Request::Command(DraftCommand::SetMode { .. } | DraftCommand::ResumeAutomatic { .. } | DraftCommand::Discard { .. }
            | DraftCommand::Commit { .. }) => None,
    };
    let stamp = SaveStamp::new(result.operation_id, result.sequence, decoded.timestamp());
    let working = match &outcome.request {
        Request::Ranges(request) => plan_incremental(previous, request,
            payload.ok_or_else(invalid)?, outcome.working.incremental().ok_or_else(invalid)?,
            stamp)?,
        Request::Command(request) => plan_change(previous, request, payload, stamp)?,
    };
    if read.is_some() && !matches!(outcome.request,
        Request::Command(DraftCommand::Commit { .. }))
    {
        return Err(invalid());
    }
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
        (Request::Command(DraftCommand::Commit { target, message, .. }), Some(content)) => {
            let selection = selection::resolve(previous, target)?;
            let record = previous.file_containers.get(&file).ok_or_else(invalid)?;
            let (_, payload) = record.working.draft().ok_or_else(invalid)?;
            content::replay(
                previous, &mut expected, target, &selection, *payload, content, message,
                principal, result.operation_id, read,
            )?;
        }
        (Request::Command(command @ DraftCommand::Commit { .. }), None) => {
            if !draft_matches_base(previous, command, read)? { return Err(invalid()); }
        }
        (Request::Command(DraftCommand::SetMode { .. } | DraftCommand::ResumeAutomatic { .. } | DraftCommand::Write { .. }
            | DraftCommand::Discard { .. }) | Request::Ranges(_), None) => {}
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
