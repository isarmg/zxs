//! A fresh displayed disk enters the same object move with an absent target
//! repository. Its root is part of that operation, not a preliminary mutation.
//! Already retained original root receipts continue their exact existing move.
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
        let movement = command.movement()?;
        if movement.target_path.parent().is_some() {
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
        if !root_recorded {
            let source = storage_engine::CapturedFileMoveSource {
                entry_id: movement.source_entry_id.clone(),
                file_id: command.expected_source_file_id,
            };
            // One original operation creates the empty root and moves the unit.
            // A replay reaches the same core before querying today's contents.
            return engine.move_object_from_file(&movement, &source, None,
                &current.principal, command.operation_id).map_err(error);
        }
        // A root receipt already retained by this exact original SMB request is
        // resumed in place. Never create a new preliminary root for a new move.
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
        current.smb_binding = Some(target_binding);
        current = current_protected_identity(&engine, &current)?;
        let source = storage_engine::CapturedFileMoveSource {
            entry_id: movement.source_entry_id,
            file_id: command.expected_source_file_id,
        };
        let original = storage_engine::FileHistoryMovePathRequest {
            source_repository_id: command.source_binding.repository_id,
            source_path: command.source_path.as_str().into(),
            target_repository_id: created.repository_id,
            target_name: command.target_name.as_str().into(),
        };
        // The retained preliminary root belongs to the original file workflow.
        // Resume that original request; do not rewrite its None intent into a
        // new object operation with a different target-repository fact.
        engine.move_current_file_captured(&original, &source, None,
            &current.principal, command.operation_id).map_err(error)
    }
}
