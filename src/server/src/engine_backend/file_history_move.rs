//! Native move owns both histories. This adapter never stages copy plus delete.
use super::*;
mod empty_target;

impl EngineBackend {
    pub(super) fn move_current_file_admitted(
        &self,
        identity: &Identity,
        command: crate::MoveCurrentFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        move_current_file_locked(&mut engine, identity, command)
    }

    pub(super) fn replace_current_file_admitted(
        &self,
        identity: &Identity,
        command: crate::ReplaceCurrentFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let replacement = storage_engine::FileHistoryMoveReplacement::parse(
            &command.expected_target_entry_id,
            command.expected_target_file_id.as_str(),
        )
        .map_err(error)?;
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        move_current_file_target_locked(&mut engine, identity, command.request, Some(&replacement))
    }
}

fn move_current_file_locked(
    engine: &mut AdmissionEngine,
    identity: &Identity,
    command: crate::MoveCurrentFile,
) -> Result<storage_contract::OperationResult, ApiError> {
    move_current_file_target_locked(engine, identity, command, None)
}

fn move_current_file_target_locked(
    engine: &mut AdmissionEngine,
    identity: &Identity,
    command: crate::MoveCurrentFile,
    replacement: Option<&storage_engine::FileHistoryMoveReplacement>,
) -> Result<storage_contract::OperationResult, ApiError> {
    let mut current = current_protected_identity(engine, identity)?;
    // Only the default share routes two separately pinned namespaces. A
    // single-repository share cannot extend its binding to another root.
    if current.smb_binding.is_some() {
        return Err(ApiError::forbidden());
    }
    for binding in [&command.source_binding, &command.target_binding] {
        current.smb_binding = Some(binding.clone());
        current = current_protected_identity(engine, &current)?;
        let repository = repo_for_branch(engine, &binding.branch_id)?;
        if !repository.file_manager_root
            || repository.repository_id != binding.repository_id
            || repository.main_branch_id != binding.branch_id
        {
            return Err(ApiError::unsupported());
        }
    }
    let source = repository(engine, &command.source_binding.repository_id)?;
    let target = repository(engine, &command.target_binding.repository_id)?;
    if source.primary_slot_id == target.primary_slot_id {
        return Err(ApiError::unsupported());
    }
    // Existing operations are reconciled by the core's original permanent
    // receipt before today's namespace. A moved-away source is not evidence
    // that the original operation failed, nor permission to capture anew.
    if !engine
        .state()
        .operations
        .contains_key(&command.operation_id)
    {
        let namespace = engine
            .namespace_tree(&command.source_binding.branch_id)
            .map_err(error)?;
        let entry = namespace
            .get_key(command.source_path.key())
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::FileNotFound,
                    "move source is no longer visible",
                )
            })?;
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::IsDirectory,
                "cross-disk move requires a single file",
            ));
        }
        if entry.entry_id != command.expected_source_entry_id
            || entry.file_id != command.expected_source_file_id.as_str()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::WorkspaceChanged,
                "move source identity changed",
            ));
        }
    }
    let request = storage_engine::FileHistoryMovePathRequest {
        source_repository_id: command.source_binding.repository_id,
        source_path: command.source_path.as_str().into(),
        target_repository_id: command.target_binding.repository_id,
        target_name: command.target_name.as_str().into(),
    };
    match replacement {
        Some(replacement) => engine.replace_current_file(
            &request,
            replacement,
            &current.principal,
            command.operation_id,
        ),
        None => engine.move_current_file(&request, &current.principal, command.operation_id),
    }
    .map_err(error)
}
