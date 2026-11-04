//! One admitted coordinator, never a first-disk initialization or removal path.
use super::*;
use crate::management::storage_addition::{ArchiveBinding, StoredAddition, configuration_digest};
use storage_contract::{OperationResult, storage_addition::AdditionPhase};

impl EngineBackend {
    pub(crate) fn addition_configuration(
        &self,
        identity: &Identity,
    ) -> Result<storage_contract::Config, ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        Ok(engine.config().clone())
    }

    pub(crate) fn run_storage_addition(
        &self,
        identity: &Identity,
        intent: &StoredAddition,
        held_directories: Option<Vec<storage_engine::PendingMemberDirectory>>,
        mut progress: impl FnMut(AdditionPhase, Option<ArchiveBinding>) -> Result<(), ApiError>,
    ) -> Result<OperationResult, ApiError> {
        let mut engine = crate::lock(&self.engine);
        let actor = current_protected_identity(&engine, identity)?;
        let operation = intent.status.operation_id;
        if engine
            .membership_fence_status()
            .is_some_and(|fence| fence.operation_id != operation)
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "the engine holds a different membership operation",
            ));
        }
        let target_digest = configuration_digest(&intent.target);
        if configuration_digest(engine.config()) == target_digest {
            // Activation can publish before updating its local projection. The
            // engine reconciles that exact generation instead of fencing again.
            progress(AdditionPhase::Activating, None)?;
            let result = engine.activate_pending_membership().map_err(error)?;
            return exact_result(result, operation);
        }
        if configuration_digest(engine.config()) != intent.status.preview.configuration_digest {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::GenerationMismatch,
                "pool configuration changed; the original addition cannot be rebound",
            ));
        }
        let directories = match held_directories {
            Some(directories) => directories,
            None => reopen_directories(intent)?,
        };
        progress(AdditionPhase::Fencing, None)?;
        engine
            .begin_membership_handoff_with_directories(
                intent.target.clone(),
                &actor.principal,
                operation,
                directories,
            )
            .map_err(error)?;
        current_protected_identity(&engine, identity)?;
        progress(AdditionPhase::Archiving, None)?;
        let archive = engine
            .build_managed_membership_control_archive()
            .map_err(error)?;
        current_protected_identity(&engine, identity)?;
        let binding = ArchiveBinding {
            archive_path: archive.archive_path.clone(),
            root_hash: archive.root_hash.clone(),
            transition_hash: archive.transition_hash.clone(),
        };
        if intent
            .archive
            .as_ref()
            .is_some_and(|saved| saved != &binding)
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "managed control archive differs from the original addition",
            ));
        }
        progress(AdditionPhase::Enrolling, Some(binding))?;
        if engine.pending_membership_status().is_some() {
            engine.resume_pending_members().map_err(error)?;
        } else {
            let directories = reopen_directories(intent)?;
            engine
                .enroll_pending_members(&archive.archive_path, &archive.root_hash, directories)
                .map_err(error)?;
        }
        current_protected_identity(&engine, identity)?;
        progress(AdditionPhase::JointDecision, None)?;
        engine.resume_membership_joint_decision().map_err(error)?;
        // Each resumed stage uses current system-session authority. Revocation
        // during the archive does not authorize the final membership publication.
        current_protected_identity(&engine, identity)?;
        progress(AdditionPhase::Activating, None)?;
        exact_result(
            engine.activate_pending_membership().map_err(error)?,
            operation,
        )
    }
}

fn exact_result(
    result: OperationResult,
    operation: storage_contract::OperationId,
) -> Result<OperationResult, ApiError> {
    if result.operation_id != operation
        || result.kind != "membership-add"
        || result.commit_state != storage_contract::CommitState::Published
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "membership publication did not confirm the original addition",
        ));
    }
    Ok(result)
}

fn reopen_directories(
    intent: &StoredAddition,
) -> Result<Vec<storage_engine::PendingMemberDirectory>, ApiError> {
    intent
        .status
        .preview
        .request
        .additions
        .iter()
        .map(|addition| {
            let disk = intent
                .target
                .disks
                .iter()
                .find(|disk| disk.slot_id == addition.slot_id)
                .expect("invariant: admitted target contains every addition");
            Ok(storage_engine::PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: crate::storage_directories::reopen_addition(&addition.selection)?,
            })
        })
        .collect()
}
