//! Actual account/SMB binding followed by the core's held native source.
use crate::engine_backend::{EngineBackend, error, repo_for_branch, repository, smb_worktree};
use crate::backend_smb_native::NativeSource;
use crate::{ApiError, Identity, SmbNativeEdit, SmbNativeHandle, SmbNativeOpen};
use name_policy::PortablePath;
use storage_contract::file_draft::{BirthRequest, DraftLocation};
use storage_contract::{OperationId, Principal};
use storage_engine::{Engine, NativeCurrentEntry};

fn authorize(
    engine: &Engine, identity: &Identity, target: &DraftLocation,
) -> Result<Principal, ApiError> {
    let binding = identity.smb_binding.as_ref().ok_or_else(ApiError::forbidden)?;
    let repo = repository(engine, &target.repository.to_string())?;
    if binding.repository_id != repo.repository_id || binding.branch_id != repo.main_branch_id
        || !repo.file_manager_root
    {
        return Err(ApiError::forbidden());
    }
    identity.principal.parse().map_err(ApiError::invalid_authenticated_principal)
}

pub(super) fn open(
    backend: &EngineBackend, identity: &Identity, request: SmbNativeOpen<'_>,
) -> Result<Option<SmbNativeHandle>, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    let repo = repo_for_branch(&engine, request.branch)?.clone();
    if !repo.file_manager_root { return Ok(None); }
    let binding = current.smb_binding.as_ref().ok_or_else(ApiError::forbidden)?;
    if binding.repository_id != repo.repository_id || binding.branch_id != request.branch
        || repo.main_branch_id != request.branch
    {
        return Err(ApiError::forbidden());
    }
    let repository = repo.repository_id.parse().map_err(ApiError::contract_input)?;
    let entries = engine.native_current_entries(repository).map_err(error)?;
    let mut published = None;
    let target = match entries.into_iter().find(|entry| match entry {
        NativeCurrentEntry::Published(entry) => entry.path.key() == request.path.key(),
        NativeCurrentEntry::Unborn { target, .. } => target.path.key() == request.path.key(),
    }) {
        Some(NativeCurrentEntry::Published(entry)) => {
            if entry.kind != storage_contract::NamespaceEntryKind::File { return Ok(None); }
            let target = DraftLocation {
                repository, path: entry.path.clone(),
                entry: entry.entry_id.parse().map_err(ApiError::contract_input)?,
                file: entry.file_id.parse().map_err(ApiError::contract_input)?,
            };
            let working = &engine.state().file_containers.get(&target.file)
                .ok_or_else(ApiError::forbidden)?.working;
            if working.draft().is_some() && working.incremental().is_none()
            {
                return Ok(None);
            }
            if request.create && request.exclusive {
                return Err(ApiError::conflict(storage_contract::ErrorCode::NameConflict,
                    "entry already exists"));
            }
            let base = engine.file_draft_base(&target).map_err(error)?;
            let version = engine.state().versions.get(&base.version_id().to_string())
                .ok_or_else(ApiError::forbidden)?;
            let mtime = crate::backend_read::version_mtime(version)?;
            published = Some((working.draft().is_none().then_some(base.version_id()), mtime));
            target
        }
        Some(NativeCurrentEntry::Unborn { target, .. }) => {
            if request.create && request.exclusive {
                return Err(ApiError::conflict(storage_contract::ErrorCode::NameConflict,
                    "entry already exists"));
            }
            target
        }
        None if !request.create => return Ok(None),
        None => {
            let birth = BirthRequest {
                repository,
                path: PortablePath::try_from(request.path.clone())
                    .map_err(ApiError::invalid_source)?,
                expected_namespace_head: engine.state().branches[request.branch]
                    .head_commit_id.parse().map_err(ApiError::contract_input)?,
            };
            let principal = current.principal.parse()
                .map_err(ApiError::invalid_authenticated_principal)?;
            engine.create_unborn_file(&birth, &principal,
                OperationId::generate().map_err(ApiError::contract_input)?)
                .map_err(error)?;
            engine.native_current_entries(repository).map_err(error)?.into_iter()
                .find_map(|entry| match entry {
                    NativeCurrentEntry::Unborn { target, .. } if target.path == birth.path => {
                        Some(target)
                    }
                    NativeCurrentEntry::Published(_) | NativeCurrentEntry::Unborn { .. } => None,
                }).ok_or_else(ApiError::forbidden)?
        }
    };
    let principal = authorize(&engine, &current, &target)?;
    let mut held = engine.open_native_working(&target, &principal, request.writable)
        .map_err(error)?;
    if request.truncate {
        engine.resize_native_working(&mut held, &principal, 0).map_err(error)?;
    }
    let (version, mtime) = published.unwrap_or((None,
        smb_protocol::protocol::FileMtime::Unpublished {}));
    Ok(Some(SmbNativeHandle {
        source: NativeSource::Current { target, held, version, mtime },
    }))
}

pub(super) fn stat(
    backend: &EngineBackend, identity: &Identity, branch: &str, path: &name_policy::LookupPath,
) -> Result<Option<smb_protocol::protocol::StatResult>, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    let repo = repo_for_branch(&engine, branch)?;
    if !repo.file_manager_root { return Ok(None); }
    let repository = repo.repository_id.parse().map_err(ApiError::contract_input)?;
    for entry in engine.native_current_entries(repository).map_err(error)? {
        if let NativeCurrentEntry::Unborn { target, working } = entry
            && target.path.key() == path.key()
        {
            authorize(&engine, &current, &target)?;
            return Ok(Some(smb_protocol::protocol::StatResult {
                entry_id: target.entry.to_string(),
                kind: storage_contract::NamespaceEntryKind::File,
                name: target.path.file_name().into(), path: target.path.into_string(),
                size: working.logical_size(),
                mtime: smb_protocol::protocol::FileMtime::Unpublished {},
            }));
        }
    }
    Ok(None)
}

pub(super) fn size(
    backend: &EngineBackend, identity: &Identity, file: &SmbNativeHandle,
) -> Result<u64, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_source(&engine, &current, &file.source)?;
    engine.native_working_size(held(&file.source)).map_err(error)
}

pub(super) fn read(
    backend: &EngineBackend, identity: &Identity, file: &SmbNativeHandle,
    offset: u64, length: u32,
) -> Result<bytes::Bytes, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_source(&engine, &current, &file.source)?;
    if length as usize > smb_protocol::protocol::MAX_READ_BYTES {
        return Err(ApiError::limit("native working read exceeds frame budget"));
    }
    let mut output = vec![0; length as usize];
    let read = engine.read_native_working(held(&file.source), &mut output, offset).map_err(error)?;
    output.truncate(read);
    Ok(output.into())
}

pub(super) fn edit(
    backend: &EngineBackend, identity: &Identity, file: &mut SmbNativeHandle,
    edit: SmbNativeEdit<'_>,
) -> Result<usize, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    let principal = authorize_source(&engine, &current, &file.source)?;
    match edit {
        SmbNativeEdit::Write { offset, bytes } => {
            engine.write_native_working(held_mut(&mut file.source), &principal, offset, bytes)
                .map_err(error)
        }
        SmbNativeEdit::Append { bytes } => {
            engine.append_native_working(held_mut(&mut file.source), &principal, bytes)
                .map_err(error)
        }
        SmbNativeEdit::Resize { length } => {
            engine.resize_native_working(held_mut(&mut file.source), &principal, length)
                .map_err(error)?;
            Ok(0)
        }
    }
}

pub(super) fn flush(
    backend: &EngineBackend, identity: &Identity, file: &mut SmbNativeHandle,
) -> Result<Option<storage_contract::OperationResult>, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    let principal = authorize_source(&engine, &current, &file.source)?;
    engine.flush_native_working(held_mut(&mut file.source), &principal).map_err(error)
}

fn authorize_source(
    engine: &Engine, identity: &Identity, source: &NativeSource,
) -> Result<Principal, ApiError> {
    match source {
        NativeSource::Current { target, .. } => authorize(engine, identity, target),
        NativeSource::Worktree { target, .. } => {
            smb_worktree::authorize_target(engine, identity, target.worktree)?;
            identity.principal.parse().map_err(ApiError::invalid_authenticated_principal)
        }
    }
}

fn held(source: &NativeSource) -> &storage_engine::NativeWorkingFile {
    match source {
        NativeSource::Current { held, .. } | NativeSource::Worktree { held, .. } => held,
    }
}

fn held_mut(source: &mut NativeSource) -> &mut storage_engine::NativeWorkingFile {
    match source {
        NativeSource::Current { held, .. } | NativeSource::Worktree { held, .. } => held,
    }
}
