//! Current admission surrounds capture, every body read and final disclosure.
use crate::{
    ApiError, Backend, DirectorySnapshotSourcePreview, ReadDirectorySnapshotSource,
    auth::Identity,
    backend_directory_snapshot_source::{
        DirectorySnapshotSourceContent, DirectorySnapshotSourceEntry, DirectorySnapshotSourceUsage,
        Initialization,
    },
    engine_backend::{EngineBackend, error_registry, error_response, repo_for_branch},
};
use storage_contract::{ErrorCode, history_window::HistoryWindowCancellation};
use storage_engine::{
    DirectorySnapshotContent, DirectorySnapshotLimits, DirectorySnapshotPreview,
    DirectorySnapshotSourceError, DirectorySnapshotVerificationError, Engine,
};

pub(super) fn read(
    backend: &EngineBackend,
    caller: &Identity,
    request: ReadDirectorySnapshotSource,
    cancellation: HistoryWindowCancellation,
) -> Result<DirectorySnapshotSourcePreview, ApiError> {
    let plan = {
        let (engine, current) = backend.lock_current(caller)?;
        authorize_scope(&engine, &current, &request)?;
        engine
            .prepare_directory_snapshot_source(
                request.branch_id,
                request.path.as_ref(),
                DirectorySnapshotLimits::default(),
                cancellation,
            )
            .map_err(source_error)?
    };
    let verification = plan
        .verify(|version| backend.authorize_version(caller, &version.to_string()))
        .map_err(|failure| match failure {
            DirectorySnapshotVerificationError::Source(source) => source_error(source),
            DirectorySnapshotVerificationError::Authorization(source) => source,
        });
    let (engine, current) = backend.lock_current(caller)?;
    authorize_scope(&engine, &current, &request)?;
    engine
        .finish_directory_snapshot_preview(verification?)
        .map(preview)
        .map_err(source_error)
}

fn authorize_scope(
    engine: &Engine,
    identity: &Identity,
    request: &ReadDirectorySnapshotSource,
) -> Result<(), ApiError> {
    let branch = request.branch_id.to_string();
    let repository = repo_for_branch(engine, &branch)?;
    if !repository.smb_enabled {
        return Err(ApiError::conflict(
            ErrorCode::SmbRebindRequired,
            "source namespace is not currently exposed to SMB",
        ));
    }
    // A caller cannot relabel an arbitrary historical/file branch as the live
    // SMB source. Disk-backed default shares resolve the current main namespace.
    if repository.main_branch_id != branch {
        return Err(ApiError::invalid(
            "source branch is not the current SMB namespace",
        ));
    }
    if identity.smb_binding.as_ref().is_some_and(|binding| {
        binding.repository_id != repository.repository_id || binding.branch_id != branch
    }) {
        return Err(ApiError::forbidden());
    }
    if !engine
        .registered_disks()
        .any(|disk| disk.slot_id == repository.primary_slot_id && disk.level == 1)
    {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

fn source_error(source: DirectorySnapshotSourceError) -> ApiError {
    let code = match &source {
        DirectorySnapshotSourceError::InvalidLimits => ErrorCode::InvalidRequest,
        DirectorySnapshotSourceError::Budget(_) => ErrorCode::ResourceLimitExceeded,
        DirectorySnapshotSourceError::Cancelled | DirectorySnapshotSourceError::Expired => {
            ErrorCode::RequestTimeout
        }
        DirectorySnapshotSourceError::AdmissionUnavailable => ErrorCode::InternalError,
        DirectorySnapshotSourceError::DirectoryUnavailable => ErrorCode::DirectoryNotFound,
        DirectorySnapshotSourceError::ManagementConflict(_) => ErrorCode::NameCollision,
        DirectorySnapshotSourceError::SourceChanged => ErrorCode::SourceChanged,
        DirectorySnapshotSourceError::ContentUnavailable(_) => ErrorCode::ContentMissing,
        DirectorySnapshotSourceError::ContentIdentity
        | DirectorySnapshotSourceError::PayloadMismatch => ErrorCode::ContentCorrupt,
        DirectorySnapshotSourceError::Read(_) => ErrorCode::IoError,
        DirectorySnapshotSourceError::Identity(_) => ErrorCode::StateInvalid,
        DirectorySnapshotSourceError::Source(cause) => {
            return error_response(cause).with_source(source);
        }
    };
    let status = error_registry::status(code);
    let message = if status.is_server_error() {
        "source verification could not complete; administrator inspection is required".into()
    } else {
        source.to_string()
    };
    ApiError::new(status, code, message).with_source(source)
}

fn preview(source: DirectorySnapshotPreview) -> DirectorySnapshotSourcePreview {
    DirectorySnapshotSourcePreview {
        repository_id: source.repository_id,
        branch_id: source.branch_id,
        root: source.root,
        initialization: Initialization::Uninitialized,
        can_commit: false,
        physical_initialization_checked: false,
        full_content_verified: true,
        entries: source
            .entries
            .into_iter()
            .map(|entry| DirectorySnapshotSourceEntry {
                path: entry.path,
                entry_id: entry.entry_id,
                content: match entry.content {
                    DirectorySnapshotContent::Directory => {
                        DirectorySnapshotSourceContent::Directory
                    }
                    DirectorySnapshotContent::File {
                        file_id,
                        version_id,
                        size,
                        content_hash,
                    } => DirectorySnapshotSourceContent::File {
                        file_id,
                        version_id,
                        size,
                        content_hash,
                    },
                },
            })
            .collect(),
        usage: DirectorySnapshotSourceUsage {
            entries: source.usage.entries,
            path_bytes: source.usage.path_bytes,
            manifest_bytes: source.usage.manifest_bytes,
            unique_versions: source.usage.unique_versions,
            route_references: source.usage.route_references,
            logical_bytes: source.usage.logical_bytes,
            proof_bytes: source.usage.proof_bytes,
            scanned_entries: source.usage.scanned_entries,
            work_units: source.usage.work_units,
        },
    }
}
