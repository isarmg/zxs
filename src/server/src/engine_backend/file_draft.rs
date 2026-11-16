//! Current management admission precedes the actual durable Draft operation.
use super::{EngineBackend, application::Application, error, file_manager, repository};
use crate::{ApiError, ChangeFileWorking, Content, FileWorkingSnapshot, ReadFileDraft, UploadSource};
use crate::auth::Identity;
use std::io::{Read, Seek, SeekFrom};
use storage_contract::file_draft::{DraftCommand, DraftLocation};

fn authorize(
    engine: &mut storage_engine::Engine, current: &Identity, target: &DraftLocation,
) -> Result<(), ApiError> {
    if current.smb_binding.is_some() { return Err(ApiError::forbidden()); }
    Application::new(engine, current).authorize_file(&target.file.to_string())?;
    let requested = repository(engine, &target.repository.to_string())?;
    let actual = file_manager::file_repo(engine, &target.file.to_string())?;
    if actual.repository_id != requested.repository_id { return Err(ApiError::forbidden()); }
    // No selection, revision or working-state lookup here. A permanent prior
    // operation must reach the engine's Existing branch before dynamic CAS.
    Ok(())
}

pub(super) fn status(
    backend: &EngineBackend, identity: &Identity, target: DraftLocation,
) -> Result<FileWorkingSnapshot, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize(&mut engine, &current, &target)?;
    Ok(FileWorkingSnapshot {
        base: engine.file_draft_base(&target).map_err(error)?,
        working: engine.file_working_state(&target).map_err(error)?,
    })
}

pub(super) fn authorize_write(
    backend: &EngineBackend, identity: &Identity, target: &DraftLocation, size: u64,
) -> Result<(), ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize(&mut engine, &current, target)?;
    if size > engine.config().max_file_bytes {
        return Err(ApiError::limit("draft exceeds the configured file limit"));
    }
    Ok(())
}

pub(super) fn change(
    backend: &EngineBackend, identity: &Identity, request: ChangeFileWorking,
) -> Result<storage_contract::OperationResult, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize(&mut engine, &current, request.command.target())?;
    if matches!(request.command, DraftCommand::Write { .. }) {
        return Err(ApiError::invalid("draft writes require the streamed body endpoint"));
    }
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    engine.change_file_working(&request.command, &principal, request.operation_id).map_err(error)
}

pub(super) fn write(
    backend: &EngineBackend, identity: &Identity, request: ChangeFileWorking,
    source: &UploadSource,
) -> Result<storage_contract::OperationResult, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize(&mut engine, &current, request.command.target())?;
    let DraftCommand::Write { content_hash, logical_size, .. } = &request.command else {
        return Err(ApiError::invalid("body endpoint requires a draft write command"));
    };
    if source.hash != *content_hash || source.size != *logical_size {
        return Err(ApiError::invalid("draft upload differs from its command"));
    }
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    let file = source.file.try_clone().map_err(ApiError::io)?;
    engine.write_file_draft(&request.command, &principal, request.operation_id, file.into())
        .map_err(error)
}

pub(super) fn content(
    backend: &EngineBackend, identity: &Identity, request: ReadFileDraft,
) -> Result<Content, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize(&mut engine, &current, &request.target)?;
    let working = engine.file_working_state(&request.target).map_err(error)?;
    let payload = *working.draft().ok_or_else(|| ApiError::new(
        http::StatusCode::CONFLICT, storage_contract::ErrorCode::FileBranchConflict,
        "selected file has no shared draft body",
    ))?.1;
    const CHUNK: u32 = 65_536;
    let count = usize::try_from(payload.logical_size().div_ceil(u64::from(CHUNK)))
        .map_err(|source| {
            ApiError::limit("draft checksum proof is too large").with_source(source)
        })?;
    let encoded_bytes = count.checked_mul(32)
        .and_then(|bytes| bytes.checked_add(storage_contract::CHECKSUM_HEADER_BYTES))
        .ok_or_else(|| ApiError::limit("draft checksum proof length overflow"))?;
    let proof_memory = crate::content_proof_budget::reserve(encoded_bytes)?;
    if std::time::Instant::now() >= request.deadline {
        return Err(crate::transfer_limits::timeout_error());
    }
    let mut file = engine.open_file_draft(&request.target).map_err(error)?;
    drop(engine);
    // The core API returns its fully verified held descriptor. Build range
    // proofs from that same descriptor and compare the complete digest again;
    // neither a new pathname nor unchecked raw-None streaming is accepted.
    file.seek(SeekFrom::Start(0)).map_err(ApiError::io)?;
    let mut chunks = Vec::with_capacity(count);
    let mut buffer = vec![0u8; CHUNK as usize];
    let mut remaining = payload.logical_size();
    let mut digest = blake3::Hasher::new();
    while remaining > 0 {
        if std::time::Instant::now() >= request.deadline {
            return Err(crate::transfer_limits::timeout_error());
        }
        file.check_running().map_err(error)?;
        let length = remaining.min(u64::from(CHUNK)) as usize;
        file.read_exact(&mut buffer[..length]).map_err(ApiError::io)?;
        digest.update(&buffer[..length]);
        chunks.push(*blake3::hash(&buffer[..length]).as_bytes());
        remaining -= length as u64;
    }
    if file.read(&mut buffer[..1]).map_err(ApiError::io)? != 0
        || storage_contract::ContentHash::from_digest(digest.finalize()) != payload.content_hash()
    {
        return Err(ApiError::new(http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::ChecksumMismatch, "draft body changed during read"));
    }
    file.check_running().map_err(error)?;
    file.seek(SeekFrom::Start(0)).map_err(ApiError::io)?;
    Ok(Content {
        fallback_files: Vec::new(),
        checksum_manifest: Some(storage_contract::ChecksumManifest {
            chunk_bytes: CHUNK, logical_size: payload.logical_size(), chunks,
        }),
        file: file.into(), size: payload.logical_size(), hash: payload.content_hash().to_string(),
        lease: Box::new(proof_memory), download_name: None,
    })
}
