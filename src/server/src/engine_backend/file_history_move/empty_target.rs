//! A fresh displayed disk has no repository binding yet. Initialize only its
//! empty control namespace, then run the existing complete file move under the
//! same owner lock. Initialization alone never removes the source or means move
//! success; its deterministic original receipt is retained for explicit retry.
use super::*;
use json_codec::json;

fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::WorkspaceChanged,
        "empty move destination or original source binding changed",
    )
}

impl EngineBackend {
    pub(in crate::engine_backend) fn move_current_file_to_empty_disk_admitted(
        &self,
        identity: &Identity,
        command: crate::MoveCurrentFileToEmptyDisk,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let mut current = current_protected_identity(&engine, identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        current.smb_binding = Some(command.source_binding.clone());
        current = current_protected_identity(&engine, &current)?;
        let source = repository(&engine, &command.source_binding.repository_id)?;
        if !source.file_manager_root
            || source.main_branch_id != command.source_binding.branch_id
            || source.primary_slot_id == command.target_slot_id
        {
            return Err(ApiError::unsupported());
        }
        let target_path = name_policy::PortablePath::parse(command.target_name.as_str())
            .map_err(ApiError::invalid_source)?;
        if target_path.parent().is_some() {
            return Err(ApiError::new(
                http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "an empty disk has no destination parent directory",
            ));
        }
        let disk = engine
            .registered_disks()
            .find(|disk| disk.slot_id == command.target_slot_id)
            .ok_or_else(changed)?;
        if disk.disk_id != command.target_disk_id
            || disk.binding_generation != command.target_binding_generation
            || disk.level != 1
            || disk.registration_mode != storage_engine::RegistrationMode::Enabled
        {
            return Err(changed());
        }
        if engine.state().repositories.values().any(|repository| {
            repository.primary_slot_id == command.target_slot_id
                && !repository.file_manager_root
                && name_policy::lookup_key(&repository.name) == target_path.key()
        }) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::NameCollision,
                "move target conflicts with a named repository",
            ));
        }
        let digest = storage_contract::object_hash(
            "smb-file-history-move-empty-root",
            &json!([
                command.operation_id,
                current.principal,
                command.source_binding.repository_id,
                command.source_binding.branch_id,
                command.source_binding.generation.to_string(),
                command.source_path.as_str(),
                command.expected_source_entry_id,
                command.expected_source_file_id.as_str(),
                command.target_slot_id,
                command.target_disk_id,
                command.target_binding_generation.to_string(),
                command.target_name.as_str()
            ]),
        )
        .map_err(ApiError::contract_input)?;
        let root_operation: storage_contract::OperationId = format!("op_{}", &digest[..32])
            .parse()
            .map_err(ApiError::contract_input)?;
        let root_recorded = engine.state().operations.contains_key(&root_operation);
        let published = engine
            .state()
            .operations
            .contains_key(&command.operation_id);
        if (!root_recorded && published)
            || (!root_recorded
                && engine.state().repositories.values().any(|repository| {
                    repository.file_manager_root
                        && repository.primary_slot_id == command.target_slot_id
                }))
        {
            return Err(changed());
        }
        if !published {
            let namespace = engine
                .namespace_tree(&command.source_binding.branch_id)
                .map_err(error)?;
            let entry = namespace
                .get_key(command.source_path.key())
                .ok_or_else(changed)?;
            if entry.kind != storage_contract::NamespaceEntryKind::File
                || entry.entry_id != command.expected_source_entry_id
                || entry.file_id != command.expected_source_file_id.as_str()
            {
                return Err(changed());
            }
        }
        let created = engine
            .create_disk_root(&command.target_slot_id, &current.principal, root_operation)
            .map_err(error)?;
        let live = repository(&engine, &created.repository_id)?;
        if !created.file_manager_root
            || live.primary_slot_id != command.target_slot_id
            || !live.smb_enabled
            || live.main_branch_id != created.main_branch_id
            || live.smb_binding_generation != created.smb_binding_generation
        {
            return Err(changed());
        }
        let target_binding = crate::auth::SmbBinding {
            repository_id: live.repository_id.clone(),
            branch_id: live.main_branch_id.clone(),
            generation: live.smb_binding_generation,
        };
        // Recheck both real bindings and source entry through the same ordinary
        // native adapter. No guessed branch and no second deletion operation.
        move_current_file_locked(
            &mut engine,
            identity,
            crate::MoveCurrentFile {
                source_binding: command.source_binding,
                target_binding,
                source_path: command.source_path,
                target_name: command.target_name,
                expected_source_entry_id: command.expected_source_entry_id,
                expected_source_file_id: command.expected_source_file_id,
                operation_id: command.operation_id,
            },
        )
    }
}
