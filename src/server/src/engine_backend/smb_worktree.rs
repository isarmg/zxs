//! Live SMB identity and pinned main binding enclose every worktree operation.
use crate::engine_backend::{EngineBackend, file_draft, repo_for_branch, repository};
use crate::engine_backend::snapshot_worktree::error;
use crate::{
    ApiError, Content, Identity, SmbWorktreeAccess, SmbWorktreeCursor, SmbWorktreeEntry,
    SmbWorktreeFile, SmbWorktreeLocation, SmbWorktreePage, UploadSource,
    SmbNativeHandle, SmbWorktreeEntryContent, SmbWorktreeWorkingOpen, SmbWorktreeWorkingSource,
};
use name_policy::{LookupName, LookupPath};
use storage_contract::snapshot_worktree::{
    EditWorktree, WorktreeHandle, WorktreeRevision, WorktreeStatus,
};
use storage_contract::RepositoryId;
use storage_engine::{Engine, WorktreeEntry, WorktreeEntryContent};

fn authorize(engine: &Engine, current: &Identity, owner: RepositoryId) -> Result<(), ApiError> {
    let binding = current.smb_binding.as_ref().ok_or_else(ApiError::forbidden)?;
    let repo = repository(engine, &owner.to_string())?;
    if !repo.file_manager_root || binding.repository_id != repo.repository_id
        || binding.branch_id != repo.main_branch_id
    {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

pub(super) fn authorize_target(
    engine: &Engine, current: &Identity, target: WorktreeHandle,
) -> Result<(), ApiError> {
    authorize(engine, current, engine.directory_worktree_owner(target).map_err(error)?)
}

fn entry(engine: &Engine, value: WorktreeEntry) -> Result<SmbWorktreeEntry, ApiError> {
    let content = match value.content {
        WorktreeEntryContent::Directory => SmbWorktreeEntryContent::Directory,
        WorktreeEntryContent::File { size, .. } => SmbWorktreeEntryContent::File { size },
        WorktreeEntryContent::Working { target, working } => {
            SmbWorktreeEntryContent::Working { target, working }
        }
        WorktreeEntryContent::RetainedFile { target, version, size, .. } => {
            let (metadata, _) = crate::engine_backend::authorized_version(
                engine, &version.to_string(),
            )?;
            SmbWorktreeEntryContent::RetainedFile {
                target, version, size, mtime: crate::backend_read::version_mtime(metadata)?,
            }
        }
    };
    Ok(SmbWorktreeEntry { path: value.path, content })
}

pub(super) fn open_working(
    backend: &EngineBackend, identity: &Identity, request: SmbWorktreeWorkingOpen,
) -> Result<SmbNativeHandle, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    let target = match &request.source {
        SmbWorktreeWorkingSource::Existing(target) => target.worktree,
        SmbWorktreeWorkingSource::Create(request) => request.target,
    };
    authorize_target(&engine, &current, target)?;
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    let target = match request.source {
        SmbWorktreeWorkingSource::Existing(target) => target,
        SmbWorktreeWorkingSource::Create(request) => {
            engine.create_directory_worktree_file(&request, &principal).map_err(error)?
        }
    };
    let mut held = engine.open_directory_worktree_working(&target, &principal, request.writable)
        .map_err(error)?;
    if request.truncate {
        engine.resize_native_working(&mut held, &principal, 0)
            .map_err(crate::engine_backend::error)?;
    }
    Ok(SmbNativeHandle {
        source: crate::backend_smb_native::NativeSource::Worktree { target, held },
    })
}

pub(super) fn mounts(
    backend: &EngineBackend, identity: &Identity, branch: &str,
) -> Result<Vec<WorktreeStatus>, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    let repo = repo_for_branch(&engine, branch)?;
    if !repo.file_manager_root || repo.main_branch_id != branch { return Ok(Vec::new()); }
    let owner = repo.repository_id.parse().map_err(ApiError::contract_input)?;
    authorize(&engine, &current, owner)?;
    engine.directory_worktree_mounts(owner).map_err(error)
}

pub(super) fn locate(
    backend: &EngineBackend, identity: &Identity, branch: &str, path: &LookupPath,
) -> Result<Option<SmbWorktreeLocation>, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    let repo = repo_for_branch(&engine, branch)?;
    if !repo.file_manager_root || repo.main_branch_id != branch { return Ok(None); }
    let owner = repo.repository_id.parse().map_err(ApiError::contract_input)?;
    authorize(&engine, &current, owner)?;
    let name = LookupName::parse(path.as_str().split('/').next()
        .expect("invariant: LookupPath contains a component"))
        .map_err(ApiError::invalid_source)?;
    let Some(status) = engine.directory_worktree_mount(owner, &name).map_err(error)? else {
        return Ok(None);
    };
    Ok(Some(SmbWorktreeLocation { status, path: path.strip_components(1) }))
}

pub(super) fn stat(
    backend: &EngineBackend, identity: &Identity, target: WorktreeHandle, path: &LookupPath,
) -> Result<SmbWorktreeEntry, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, target)?;
    entry(&engine, engine.directory_worktree_entry(target, path).map_err(error)?)
}

pub(super) fn page(
    backend: &EngineBackend, identity: &Identity, target: WorktreeHandle,
    parent: Option<&LookupPath>, cursor: Option<&SmbWorktreeCursor>,
) -> Result<SmbWorktreePage, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, target)?;
    let page = engine.directory_worktree_entries(target, parent, cursor.map(|value| &value.held))
        .map_err(error)?;
    Ok(SmbWorktreePage {
        stamp: page.stamp,
        entries: page.entries.into_iter().map(|value| entry(&engine, value))
            .collect::<Result<_, _>>()?,
        next: page.next.map(|held| SmbWorktreeCursor { held }),
    })
}

pub(super) fn check_page(
    backend: &EngineBackend, identity: &Identity, target: WorktreeHandle,
    parent: Option<&LookupPath>, page: &SmbWorktreePage,
) -> Result<(), ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, target)?;
    engine.check_directory_worktree_page(target, parent, &page.stamp).map_err(error)
}

pub(super) fn check(
    backend: &EngineBackend, identity: &Identity, target: WorktreeHandle,
    access: SmbWorktreeAccess<'_>,
) -> Result<(), ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, target)?;
    match access {
        SmbWorktreeAccess::Read => {
            engine.directory_worktree_status(target).map_err(error)?;
        }
        SmbWorktreeAccess::Write { path, expected } => {
            engine.check_directory_worktree_edit(target, path, expected).map_err(error)?;
        }
    }
    Ok(())
}

pub(super) fn content(
    backend: &EngineBackend, identity: &Identity, file: &SmbWorktreeFile,
    deadline: std::time::Instant,
) -> Result<Content, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, file.target)?;
    let content = engine.open_directory_worktree_content(file.target, &(&file.path).into())
        .map_err(error)?;
    if content.revision != file.revision {
        return Err(ApiError::conflict(storage_contract::ErrorCode::PreparationStale,
            "worktree changed while opening its content"));
    }
    drop(engine);
    file_draft::verified_content(content.size, content.content_hash, deadline, || Ok(content.file))
}

pub(super) fn edit(
    backend: &EngineBackend, identity: &Identity, request: EditWorktree, source: &UploadSource,
) -> Result<WorktreeRevision, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize_target(&engine, &current, request.target)?;
    if source.size != request.size || source.hash != request.content_hash {
        return Err(ApiError::conflict(storage_contract::ErrorCode::ChecksumMismatch,
            "retained SMB bytes differ from the original worktree save"));
    }
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    let source = source.file.try_clone().map_err(ApiError::io)?;
    // Exact original retries enter the engine before today's revision check.
    engine.edit_directory_worktree(&request, &principal, source.into()).map_err(error)
}
