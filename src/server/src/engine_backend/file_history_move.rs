//! Native move owns both histories. This adapter never stages copy plus delete.
use super::*;
use storage_contract::file_relocation::NamespaceEntryId;
mod empty_target;

impl EngineBackend {
    pub(super) fn move_current_object_admitted(
        &self,
        identity: &Identity,
        command: crate::MoveCurrentObject,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let mut current = current_protected_identity(&engine, identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        require_move_binding(&engine, &mut current, &command.source_binding)?;
        let source = repository(&engine, &command.source_binding.repository_id)?;
        if command.movement.source_repository_id.as_str() != command.source_binding.repository_id
            || source.primary_slot_id != command.source_slot_id
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::WorkspaceChanged,
                "move source no longer belongs to its captured namespace binding",
            ));
        }
        require_move_disk(
            &engine,
            &command.source_slot_id,
            &command.source_disk_id,
            command.source_binding_generation,
        )?;
        match &command.target {
            crate::MoveObjectTarget::Repository {
                binding,
                disk_id,
                binding_generation,
            } => {
                require_move_binding(&engine, &mut current, binding)?;
                let target = repository(&engine, &binding.repository_id)?;
                if command
                    .movement
                    .target_repository_id
                    .as_ref()
                    .map(|id| id.as_str())
                    != Some(binding.repository_id.as_str())
                    || command.movement.target_slot_id != target.primary_slot_id
                {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::WorkspaceChanged,
                        "move target differs from its captured namespace binding",
                    ));
                }
                require_move_disk(
                    &engine,
                    &command.movement.target_slot_id,
                    disk_id,
                    *binding_generation,
                )?;
            }
            crate::MoveObjectTarget::EmptyDisk {
                disk_id,
                binding_generation,
            } => {
                require_move_disk(
                    &engine,
                    &command.movement.target_slot_id,
                    disk_id,
                    *binding_generation,
                )?;
                if command.movement.target_repository_id.is_some() {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::WorkspaceChanged,
                        "move destination disk binding changed",
                    ));
                }
            }
        }
        // Captured None is checked as a directory identity in the original
        // receipt as well as a new plan. Neither retry resolves today's path.
        engine
            .move_captured_object(
                &command.movement,
                command.expected_source_file_id.as_ref(),
                command.expected_target_file_id.as_ref(),
                &current.principal,
                command.operation_id,
            )
            .map_err(error)
    }

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
        let target_entry = NamespaceEntryId::parse(&command.expected_target_entry_id)
            .map_err(ApiError::contract_input)?;
        move_current_file_target_locked(
            &mut engine,
            identity,
            command.request,
            Some(&replacement),
            Some(target_entry),
        )
    }
}

fn require_move_disk(
    engine: &AdmissionEngine,
    slot: &str,
    disk_id: &str,
    binding_generation: u64,
) -> Result<(), ApiError> {
    let disk = engine
        .registered_disks()
        .find(|disk| disk.slot_id == slot)
        .ok_or_else(|| {
            ApiError::conflict(
                storage_contract::ErrorCode::WorkspaceChanged,
                "captured move disk registration disappeared",
            )
        })?;
    if disk.disk_id != disk_id
        || disk.binding_generation != binding_generation
        || disk.level != 1
        || disk.registration_mode != storage_engine::RegistrationMode::Enabled
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::WorkspaceChanged,
            "captured move disk binding changed",
        ));
    }
    Ok(())
}

fn require_move_binding(
    engine: &AdmissionEngine,
    current: &mut Identity,
    binding: &crate::auth::SmbBinding,
) -> Result<(), ApiError> {
    current.smb_binding = Some(binding.clone());
    *current = current_protected_identity(engine, current)?;
    let repository = repo_for_branch(engine, &binding.branch_id)?;
    if repository.repository_id != binding.repository_id
        || repository.main_branch_id != binding.branch_id
    {
        return Err(ApiError::unsupported());
    }
    Ok(())
}

fn move_current_file_locked(
    engine: &mut AdmissionEngine,
    identity: &Identity,
    command: crate::MoveCurrentFile,
) -> Result<storage_contract::OperationResult, ApiError> {
    move_current_file_target_locked(engine, identity, command, None, None)
}

fn move_current_file_target_locked(
    engine: &mut AdmissionEngine,
    identity: &Identity,
    command: crate::MoveCurrentFile,
    replacement: Option<&storage_engine::FileHistoryMoveReplacement>,
    target_entry_id: Option<NamespaceEntryId>,
) -> Result<storage_contract::OperationResult, ApiError> {
    let mut current = current_protected_identity(engine, identity)?;
    // Only the default share routes two separately pinned namespaces. A
    // single-repository share cannot extend its binding to another root.
    if current.smb_binding.is_some() {
        return Err(ApiError::forbidden());
    }
    for binding in [&command.source_binding, &command.target_binding] {
        require_move_binding(engine, &mut current, binding)?;
    }
    let target_slot_id = repository(engine, &command.target_binding.repository_id)?
        .primary_slot_id
        .clone();
    let movement = command.movement(
        &target_slot_id,
        target_entry_id.as_ref().map(NamespaceEntryId::as_str),
    )?;
    let source = storage_engine::CapturedFileMoveSource {
        entry_id: movement.source_entry_id.clone(),
        file_id: command.expected_source_file_id,
    };
    // The core checks the complete captured source/replacement identities after
    // its original receipt or selected intent, before any new namespace lookup.
    // File/directory owner classification and same/cross-disk planning stay there.
    engine
        .move_object_from_file(
            &movement,
            &source,
            replacement,
            &current.principal,
            command.operation_id,
        )
        .map_err(error)
}
