//! Capture and publication recheck account/binding authorization under the
//! engine owner lock. There is no adapter-level copy/delete sequence.
use super::*;

fn require_main<'a>(
    engine: &'a Engine,
    identity: &Identity,
    branch: &str,
) -> Result<&'a storage_contract::Repository, ApiError> {
    if identity
        .smb_binding
        .as_ref()
        .is_some_and(|binding| binding.branch_id != branch)
    {
        return Err(ApiError::forbidden());
    }
    let repository = repo_for_branch(engine, branch)?;
    if repository.main_branch_id != branch {
        return Err(ApiError::unsupported());
    }
    Ok(repository)
}

impl EngineBackend {
    pub(super) fn prepare_directory_file_export_admitted(
        &self,
        identity: &Identity,
        command: crate::PrepareDirectoryFileExport,
    ) -> Result<Option<crate::DirectoryFileExport>, ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let repo = require_main(&engine, &current, &command.branch_id)?;
        engine
            .directory_file_export_request(
                &repo.repository_id,
                &command.source_path,
                &command.destination,
            )
            .map_err(error)?
            .map(|request| {
                Ok(crate::DirectoryFileExport {
                    repository_id: request.source_repository_id,
                    branch_id: command.branch_id,
                    source_path: name_policy::PortablePath::parse(&request.source_path)
                        .map_err(ApiError::invalid_source)?,
                    destination: command.destination,
                    expected_head: request.expected_source_head,
                })
            })
            .transpose()
    }

    pub(super) fn export_directory_file_admitted(
        &self,
        identity: &Identity,
        command: crate::ExportDirectoryFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let request = command.request;
        let repo = require_main(&engine, &current, &request.branch_id)?;
        if repo.repository_id != request.repository_id {
            return Err(ApiError::forbidden());
        }
        let input = storage_engine::FileHistoryCopyRequest {
            source_repository_id: request.repository_id.clone(),
            source_path: request.source_path.as_str().into(),
            expected_source_head: request.expected_head.clone(),
            target_repository_id: request.repository_id,
            target_name: request.destination.as_str().into(),
            expected_target_head: request.expected_head,
        };
        // An exact permanent receipt precedes today's path and head lookup.
        engine
            .export_directory_file_history(&input, &current.principal, command.operation_id)
            .map_err(error)
    }
}
