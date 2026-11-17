//! SMB may replace a shared draft, never change mode, discard it or commit a
//! history node. Management's separate admission remains mandatory for those.
use crate::engine_backend::{EngineBackend, error, read_projection, repo_for_branch, repository};
use crate::{ApiError, Content, Identity, SmbAutomaticFile, SmbDraftAccess, SmbManualFile};
use storage_contract::file_draft::{
    CommitMode, DraftCommand, DraftLocation, FileWorkingState, WorkingRevision,
};
use storage_contract::{Entry, ErrorCode, FileId, NamespaceEntryKind};

fn changed() -> ApiError {
    ApiError::conflict(ErrorCode::FileBranchConflict, "the open FILE working selection changed")
}

fn authorize_target(
    engine: &storage_engine::Engine,
    current: &Identity,
    target: &DraftLocation,
) -> Result<(), ApiError> {
    let binding = current.smb_binding.as_ref().ok_or_else(ApiError::forbidden)?;
    let repository = repository(engine, &target.repository.to_string())?;
    if binding.repository_id != repository.repository_id
        || binding.branch_id != repository.main_branch_id
    {
        return Err(ApiError::forbidden());
    }
    // The current identity and binding generation were admitted by lock_current.
    // Do not perform a revision/base CAS here: exact retries must first reach
    // the engine's original operation/digest/principal check.
    Ok(())
}

fn require_selection(
    engine: &storage_engine::Engine,
    file: &SmbManualFile,
    expected: storage_contract::file_draft::WorkingRevision,
) -> Result<(), ApiError> {
    if engine.file_draft_base(&file.target).map_err(error)? != file.base {
        return Err(changed());
    }
    let working = engine.file_working_state(&file.target).map_err(error)?;
    if working.mode() != CommitMode::Manual || working.revision() != expected {
        return Err(changed());
    }
    Ok(())
}

/// A current occurrence may enrich an automatic handle, but an unsupported
/// share or mode cannot produce a conversion proof.
pub(super) fn capture_automatic(
    backend: &EngineBackend,
    identity: &Identity,
    branch: &str,
    observed: &Entry,
) -> Result<Option<SmbAutomaticFile>, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    let repo = repo_for_branch(&engine, branch)?;
    if !repo.file_manager_root
        || repo.main_branch_id != branch
        || observed.kind != NamespaceEntryKind::File
    {
        return Ok(None);
    }
    let file = observed.file_id.parse().map_err(ApiError::invalid_source)?;
    let Some(record) = engine.state().file_containers.get(&file) else {
        return Ok(None);
    };
    let FileWorkingState::Automatic { revision } = &record.working else {
        return Ok(None);
    };
    let target = location(repo, observed, file)?;
    authorize_target(&engine, &current, &target)?;
    let base = match engine.file_draft_base(&target) {
        Ok(base) => base,
        Err(source)
            if matches!(
                source.code(), ErrorCode::UnsupportedOperation | ErrorCode::FileBranchConflict
            ) =>
        {
            return Ok(None);
        }
        Err(source) => return Err(error(source)),
    };
    if base.version_id().to_string() != observed.version_id {
        return Ok(None);
    }
    Ok(Some(SmbAutomaticFile { base, target, revision: *revision }))
}

pub(super) fn require_automatic_revision(
    engine: &storage_engine::Engine,
    current: &Identity,
    automatic: &SmbAutomaticFile,
) -> Result<(), ApiError> {
    authorize_target(engine, current, &automatic.target)?;
    let record = engine.state().file_containers.get(&automatic.target.file)
        .ok_or_else(changed)?;
    if let FileWorkingState::Automatic { revision } = &record.working
        && *revision == automatic.revision
    {
        return Ok(());
    }
    Err(changed())
}

pub(super) fn switched_revision(
    backend: &EngineBackend,
    identity: &Identity,
    automatic: &SmbAutomaticFile,
) -> Result<Option<WorkingRevision>, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, &automatic.target)?;
    let record = engine.state().file_containers.get(&automatic.target.file)
        .ok_or_else(changed)?;
    let revision = match &record.working {
        FileWorkingState::Automatic { .. } => return Ok(None),
        FileWorkingState::ManualClean { revision } => *revision,
        FileWorkingState::Unborn { .. } | FileWorkingState::ManualDirty { .. }
        | FileWorkingState::ManualIncremental { .. } => return Err(changed()),
    };
    let next = automatic.revision.next().map_err(|source| {
        ApiError::limit("the shared draft revision is exhausted")
            .with_source(crate::http_boundary::ApiErrorSource::WorkingRevision(source))
    })?;
    if next != revision
        || engine.file_draft_base(&automatic.target).map_err(error)? != automatic.base
    {
        return Err(changed());
    }
    Ok(Some(revision))
}

fn observed_occurrence(
    engine: &storage_engine::Engine,
    branch: &str,
    observed: &Entry,
) -> Result<Option<Entry>, ApiError> {
    let actual = read_projection::lookup_current(
        engine, branch, &name_policy::LookupPath::from(&observed.path),
    )?;
    Ok(actual.filter(|actual| {
        actual.kind == NamespaceEntryKind::File
            && actual.entry_id == observed.entry_id
            && actual.file_id == observed.file_id
            && actual.version_id == observed.version_id
            && actual.path == observed.path
    }))
}

fn location(
    repository: &storage_contract::Repository,
    actual: &Entry,
    file: FileId,
) -> Result<DraftLocation, ApiError> {
    Ok(DraftLocation {
        repository: repository.repository_id.parse().map_err(ApiError::invalid_source)?,
        path: actual.path.clone(),
        entry: actual.entry_id.parse().map_err(ApiError::invalid_source)?,
        file,
    })
}

pub(super) fn capture(
    backend: &EngineBackend,
    identity: &Identity,
    branch: &str,
    observed: &Entry,
) -> Result<Option<SmbManualFile>, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    let actual = observed_occurrence(&engine, branch, observed)?.ok_or_else(changed)?;
    let file = actual.file_id.parse().map_err(ApiError::invalid_source)?;
    let record = engine.state().file_containers.get(&file).ok_or_else(changed)?;
    if record.working.mode() == CommitMode::Automatic {
        return Ok(None);
    }
    let repository = repo_for_branch(&engine, branch)?;
    let target = location(repository, &actual, file)?;
    authorize_target(&engine, &current, &target)?;
    if repository.main_branch_id != branch {
        return Err(ApiError::forbidden());
    }
    Ok(Some(SmbManualFile {
        base: engine.file_draft_base(&target).map_err(error)?,
        working: engine.file_working_state(&target).map_err(error)?,
        target,
    }))
}

pub(super) fn authorize(
    backend: &EngineBackend,
    identity: &Identity,
    file: &SmbManualFile,
    access: SmbDraftAccess,
) -> Result<(), ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, &file.target)?;
    match access {
        SmbDraftAccess::Write { expected } => require_selection(&engine, file, expected),
        SmbDraftAccess::Read => {
            let repository = repository(&engine, &file.target.repository.to_string())?;
            let entry = read_projection::lookup_current(
                &engine, &repository.main_branch_id,
                &name_policy::LookupPath::from(&file.target.path),
            )?.ok_or_else(changed)?;
            if entry.entry_id != file.target.entry.to_string()
                || entry.file_id != file.target.file.to_string()
            {
                return Err(changed());
            }
            Ok(())
        }
    }
}

pub(super) fn content(
    backend: &EngineBackend,
    identity: &Identity,
    file: &SmbManualFile,
    deadline: std::time::Instant,
) -> Result<Content, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, &file.target)?;
    require_selection(&engine, file, file.revision())?;
    let working = engine.file_working_state(&file.target).map_err(error)?;
    if let Some((_, payload)) = working.draft() {
        let payload = *payload;
        return crate::engine_backend::file_draft::saved_content(
            payload, deadline, || {
            let source = engine.open_file_draft(&file.target).map_err(error)?;
            drop(engine);
            Ok(source)
        });
    }
    let plan = engine.prepare_content(&file.base.version_id().to_string()).map_err(error)?;
    let memory = crate::content_proof_budget::reserve(
        plan.checksum_manifest_bytes().map_err(error)?,
    )?;
    drop(engine);
    let opened = plan.open().map_err(error)?;
    Ok(Content {
        checksum_manifest: Some(opened.manifest),
        fallback_files: opened.fallback_files.into_iter().map(Into::into).collect(),
        file: opened.file.into(),
        size: opened.size,
        hash: opened.content_hash.to_string(),
        lease: Box::new((opened.lease, memory)),
        download_name: None,
    })
}

pub(super) fn write(
    backend: &EngineBackend,
    identity: &Identity,
    request: crate::ChangeFileWorking,
    source: &crate::UploadSource,
) -> Result<storage_contract::OperationResult, ApiError> {
    let DraftCommand::Write { .. } = &request.command else {
        return Err(ApiError::forbidden());
    };
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, request.command.target())?;
    let principal = current.principal.parse()
        .map_err(ApiError::invalid_authenticated_principal)?;
    // The command is frozen before first publication. A lost response retries
    // that command even if the mutable FD was subsequently changed. Existing
    // returns the same receipt; a new attempt still verifies its entire BODY.
    let source = source.file.try_clone().map_err(ApiError::io)?;
    engine.write_file_draft(&request.command, &principal, request.operation_id, source.into())
        .map_err(error)
}

/// Directory cursors keep the draft size observed at capture, just as they keep
/// the selected historical entry. Later paging never mutates a frozen cursor.
pub(super) fn directory_entries(
    engine: &storage_engine::Engine,
    branch: &str,
    entries: crate::backend_read::DirectoryEntries,
) -> Result<crate::backend_read::DirectoryEntries, ApiError> {
    let repository = repo_for_branch(engine, branch)?;
    if !repository.file_manager_root || repository.main_branch_id != branch {
        return Ok(entries);
    }
    let mut changed: Option<Vec<Entry>> = None;
    for (index, entry) in entries.as_slice().iter().enumerate() {
        let Some(size) = published_size(engine, repository, entry)? else {
            continue;
        };
        let changed = changed.get_or_insert_with(|| entries.as_slice().to_vec());
        changed[index].size = size;
    }
    Ok(match changed {
        Some(entries) => {
            crate::backend_read::DirectoryEntries::Projected(std::sync::Arc::new(entries))
        }
        None => entries,
    })
}

/// Project only an already-selected current ordinary occurrence under the same
/// admission lock. The scalar is frozen in its caller's owned page, never read
/// again by the RPC cursor. Named, historical and backup entries receive no overlay.
pub(in crate::engine_backend) fn published_size(
    engine: &storage_engine::Engine,
    repository: &storage_contract::Repository,
    entry: &Entry,
) -> Result<Option<u64>, ApiError> {
    if !repository.file_manager_root || entry.kind != NamespaceEntryKind::File {
        return Ok(None);
    }
    let file: FileId = entry.file_id.parse().map_err(ApiError::invalid_source)?;
    let Some(record) = engine.state().file_containers.get(&file) else {
        return Ok(None);
    };
    let Some((_, payload)) = record.working.draft() else {
        return Ok(None);
    };
    let target = DraftLocation {
        repository: repository.repository_id.parse().map_err(ApiError::invalid_source)?,
        path: entry.path.clone(),
        entry: entry.entry_id.parse().map_err(ApiError::invalid_source)?,
        file,
    };
    engine.file_draft_base(&target).map_err(error)?;
    Ok(Some(payload.logical_size()))
}
