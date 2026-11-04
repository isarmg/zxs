//! Current registration identity and real exit-generation publication only.
use super::*;
use crate::management::{
    storage_addition::{ArchiveBinding, configuration_digest, digest},
    storage_lifecycle::StoredExit,
};
use storage_contract::{Config, OperationResult, storage_lifecycle::*};
use storage_engine::{DiskExitPhase, MembershipExitAction, RegistrationMode};

fn engine_action(action: ExitAction) -> MembershipExitAction {
    match action {
        ExitAction::PlannedOffline => MembershipExitAction::PlannedOffline,
        ExitAction::Deregister => MembershipExitAction::Deregister,
    }
}

impl EngineBackend {
    pub(crate) fn preview_storage_exit(
        &self,
        identity: &Identity,
        request: ExitRequest,
    ) -> Result<(ExitPreview, Config), ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        preview(&engine, request)
    }

    pub(crate) fn storage_exit_result(
        &self,
        identity: &Identity,
        intent: &StoredExit,
    ) -> Result<Option<OperationResult>, ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        published_result(&engine, identity, intent)
    }

    pub(crate) fn run_storage_exit(
        &self,
        identity: &Identity,
        intent: &StoredExit,
        mut progress: impl FnMut(ExitPhase, Option<ArchiveBinding>) -> Result<(), ApiError>,
    ) -> Result<OperationResult, ApiError> {
        let mut engine = crate::lock(&self.engine);
        let actor = current_protected_identity(&engine, identity)?;
        if let Some(result) = published_result(&engine, identity, intent)? {
            return Ok(result);
        }
        if let Some(status) = engine.disk_exit_status().map_err(error)?
            && status.operation_id == intent.status.operation_id
        {
            verify_status(&status, intent)?;
            if matches!(
                status.phase,
                DiskExitPhase::Activating | DiskExitPhase::Published
            ) {
                // The actual engine phase, never the HTTP phase cache, decides
                // whether origin preparation may still touch the target.
                progress(ExitPhase::Activating, None)?;
                current_protected_identity(&engine, identity)?;
                return exact_result(
                    engine.activate_pending_disk_exit().map_err(error)?,
                    intent,
                    intent.archive.as_ref(),
                );
            }
        } else {
            let (current, _) = preview(&engine, intent.status.preview.request.clone())?;
            if current != intent.status.preview {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::GenerationMismatch,
                    "exit preview no longer matches the current registration and published boundary",
                ));
            }
        }
        progress(ExitPhase::Fencing, None)?;
        let fence = engine
            .begin_disk_exit(
                &intent.status.preview.target.disk_id,
                engine_action(intent.status.preview.request.action),
                &actor.principal,
                intent.status.operation_id,
            )
            .map_err(error)?;
        if fence.operation_id != intent.status.operation_id {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "exit fence differs from the original operation",
            ));
        }
        current_protected_identity(&engine, identity)?;
        progress(ExitPhase::Archiving, None)?;
        let archive = engine
            .build_managed_membership_control_archive()
            .map_err(error)?;
        let binding = ArchiveBinding {
            archive_path: archive.archive_path,
            root_hash: archive.root_hash,
            transition_hash: archive.transition_hash,
        };
        if binding.transition_hash != fence.transition_hash
            || intent
                .archive
                .as_ref()
                .is_some_and(|saved| saved != &binding)
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "exit control archive differs from the admitted original fence",
            ));
        }
        current_protected_identity(&engine, identity)?;
        progress(ExitPhase::Enrolling, Some(binding.clone()))?;
        if engine.pending_membership_status().is_some() {
            engine.resume_pending_members().map_err(error)?;
        } else {
            engine
                .enroll_pending_members(&binding.archive_path, &binding.root_hash, Vec::new())
                .map_err(error)?;
        }
        current_protected_identity(&engine, identity)?;
        progress(ExitPhase::JointDecision, None)?;
        engine.resume_membership_joint_decision().map_err(error)?;
        current_protected_identity(&engine, identity)?;
        progress(ExitPhase::Activating, None)?;
        exact_result(
            engine.activate_pending_disk_exit().map_err(error)?,
            intent,
            Some(&binding),
        )
    }
}

fn preview(engine: &Engine, request: ExitRequest) -> Result<(ExitPreview, Config), ApiError> {
    if engine.membership_fence_status().is_some() {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "continue the original fenced membership operation before previewing an exit",
        ));
    }
    if engine
        .state()
        .operations
        .contains_key(&request.operation_id)
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::IdempotencyKeyReused,
            "this operation_id already belongs to a published operation",
        ));
    }
    let registered = engine
        .registered_disk(&request.expected_disk_id)
        .filter(|disk| disk.slot_id == request.slot_id)
        .ok_or_else(|| {
            ApiError::conflict(
                storage_contract::ErrorCode::GenerationMismatch,
                "the selected directory is no longer this current registration",
            )
        })?;
    if registered.registration_mode == RegistrationMode::Disabled
        && request.action == ExitAction::PlannedOffline
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "this registration is already offline; deregistration remains a separate operation",
        ));
    }
    if engine
        .disk_exit_status()
        .map_err(error)?
        .is_some_and(|status| status.phase != DiskExitPhase::Published)
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "continue the original unpublished exit before starting a successor",
        ));
    }
    let original_metadata_members = engine
        .registered_disks()
        .filter(|disk| disk.metadata_member)
        .count();
    let target_metadata_members =
        original_metadata_members - usize::from(registered.metadata_member);
    if target_metadata_members == 0 {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "the final metadata voter cannot exit without terminal pool authority",
        ));
    }
    let identity = registered.identity();
    let projection = engine
        .preview_disk_deregistration(&identity)
        .map_err(deregistration_plan_error)?;
    let boundary = projection.boundary();
    let snapshot_hash = boundary.published_snapshot_hash.ok_or_else(|| {
        ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "exit requires an authenticated published source boundary",
        )
    })?;
    let config = engine.config().clone();
    let disk = config
        .disks
        .iter()
        .find(|disk| disk.disk_id == identity.disk_id)
        .expect("invariant: current registration has a historical definition");
    let mut preview = ExitPreview {
        request,
        target: ExitTarget {
            pool_id: identity.pool_id,
            disk_id: identity.disk_id,
            slot_id: identity.slot_id,
            binding_generation: identity.binding_generation,
            root_path: disk
                .root
                .to_str()
                .ok_or_else(|| ApiError::invalid("registered directory is not a UTF-8 path"))?
                .to_owned(),
        },
        configuration_digest: configuration_digest(&config),
        preview_digest: String::new(),
        source_sequence: boundary.sequence,
        topology_generation: boundary.topology_generation,
        snapshot_hash: snapshot_hash.to_owned(),
        original_metadata_members,
        target_metadata_members,
    };
    preview.preview_digest = digest(&preview);
    Ok((preview, config))
}

/// The current generation can be a later Add, Abort or Exit. Permanent published
/// receipts, not the head's lifecycle status, identify an older completed exit.
fn published_result(
    engine: &Engine,
    identity: &Identity,
    intent: &StoredExit,
) -> Result<Option<OperationResult>, ApiError> {
    let operation = intent.status.operation_id;
    if engine.config().pool_id != intent.status.preview.target.pool_id {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "exit receipt belongs to another pool",
        ));
    }
    if let Some(status) = engine.disk_exit_status().map_err(error)?
        && status.operation_id == operation
    {
        verify_status(&status, intent)?;
        // persist may already have installed the permanent result while final
        // D3 proof/current-view admission still failed. Complete that same
        // activation rather than treating a State row as the final boundary.
        if status.phase != DiskExitPhase::Published {
            return Ok(None);
        }
    }
    if !engine.state().operations.contains_key(&operation) {
        return Ok(None);
    }
    let record = engine.operation_record(operation).map_err(error)?;
    if record.owner() != Some(identity.principal.as_str()) {
        return Err(ApiError::forbidden());
    }
    if record.result().commit_state != storage_contract::CommitState::Published {
        return Ok(None);
    }
    exact_result(record.into_result(), intent, intent.archive.as_ref()).map(Some)
}

fn verify_status(
    status: &storage_engine::DiskExitStatus,
    intent: &StoredExit,
) -> Result<(), ApiError> {
    let target = &intent.status.preview.target;
    if status.operation_id != intent.status.operation_id
        || status.action != engine_action(intent.status.preview.request.action)
        || status.target.pool_id != target.pool_id
        || status.target.disk_id != target.disk_id
        || status.target.slot_id != target.slot_id
        || status.target.binding_generation != target.binding_generation
        || intent
            .archive
            .as_ref()
            .is_some_and(|binding| status.transition_hash != binding.transition_hash)
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::IdempotencyKeyReused,
            "engine exit authority differs from the original operation, action or registration",
        ));
    }
    Ok(())
}
fn exact_result(
    result: OperationResult,
    intent: &StoredExit,
    archive: Option<&ArchiveBinding>,
) -> Result<OperationResult, ApiError> {
    let target = &intent.status.preview.target;
    let registration = match intent.status.preview.request.action {
        ExitAction::PlannedOffline => "disabled",
        ExitAction::Deregister => "detached",
    };
    let binding_generation = target.binding_generation.to_string();
    let outcome = |key: &str| result.result.get(key).and_then(json_codec::Value::as_str);
    if result.operation_id != intent.status.operation_id
        || outcome("disk_id") != Some(target.disk_id.as_str())
        || outcome("slot_id") != Some(target.slot_id.as_str())
        || outcome("binding_generation") != Some(binding_generation.as_str())
        || outcome("registration") != Some(registration)
        || outcome("target_io") != Some("closed")
        || result.kind != intent.status.preview.request.action.result_kind()
        || result.commit_state != storage_contract::CommitState::Published
        || archive.is_none_or(|binding| result.request_digest != binding.transition_hash)
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "exit did not confirm the exact original published lifecycle result",
        ));
    }
    Ok(result)
}

/// Projection failure classifications remain typed through the HTTP boundary.
fn deregistration_plan_error(source: storage_engine::DeregistrationPlanError) -> ApiError {
    use storage_contract::ErrorCode;
    use storage_engine::DeregistrationPlanError;
    let code = match &source {
        DeregistrationPlanError::TargetNotRegistered(_)
        | DeregistrationPlanError::VersionNotFound(_)
        | DeregistrationPlanError::NamespaceNotFound(_) => ErrorCode::NotFound,
        DeregistrationPlanError::TargetIdentityMismatch { .. } => ErrorCode::DiskIdentityMismatch,
        DeregistrationPlanError::CurrentNamespaceUnavailable => ErrorCode::RecoveryRequired,
        DeregistrationPlanError::PlacementMissing(_)
        | DeregistrationPlanError::PlacementSlotNotRegistered { .. }
        | DeregistrationPlanError::PlacementBindingMismatch { .. }
        | DeregistrationPlanError::PlacementInvalid { .. }
        | DeregistrationPlanError::NamespaceReferenceMissing(_) => ErrorCode::ObjectCorrupt,
        DeregistrationPlanError::PageSizeOutOfRange(_) => ErrorCode::InvalidRequest,
    };
    ApiError::new(
        super::error_registry::status(code),
        code,
        source.to_string(),
    )
    .with_source(source)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/storage_lifecycle_errors.rs"]
mod errors_tests;
