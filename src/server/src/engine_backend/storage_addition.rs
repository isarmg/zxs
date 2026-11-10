//! One admitted coordinator for original pure-add publication or certified Abort.
use super::*;
use crate::management::storage_addition::{ArchiveBinding, StoredAddition, configuration_digest};
use storage_contract::{
    OperationResult,
    storage_addition::{AdditionContinuation, AdditionPhase},
};

impl EngineBackend {
    pub(crate) fn addition_root(
        &self,
        identity: &Identity,
    ) -> Result<std::path::PathBuf, ApiError> {
        let engine = crate::lock(&self.engine);
        engine.managed_identity(identity)?;
        if identity.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        Ok(engine.root().to_path_buf())
    }

    pub(crate) fn addition_configuration(
        &self,
        identity: &Identity,
    ) -> Result<storage_contract::Config, ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        Ok(engine.config().clone())
    }

    /// Current lifecycle projection only. Historical stopped paths are never
    /// opened merely to check whether a fresh selection overlaps them.
    pub(crate) fn addition_directory_scope(
        &self,
        identity: &Identity,
    ) -> Result<(storage_contract::Config, Vec<std::path::PathBuf>), ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        let stopped = stopped_roots(&engine);
        Ok((engine.config().clone(), stopped))
    }

    /// Metadata-only permanent receipt lookup. A pending Candidate is not success.
    pub(crate) fn storage_addition_result(
        &self,
        identity: &Identity,
        intent: &StoredAddition,
    ) -> Result<Option<OperationResult>, ApiError> {
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        published_result(&engine, identity, intent)
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
        if let Some(result) = published_result(&engine, identity, intent)? {
            return Ok(result);
        }
        if intent.status.continuation == AdditionContinuation::Cancel {
            return cancel_addition(&mut engine, identity, intent, &mut progress);
        }
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
            return exact_result(result, operation, intent.archive.as_ref());
        }
        if configuration_digest(engine.config()) != intent.status.preview.configuration_digest {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::GenerationMismatch,
                "pool configuration changed; the original addition cannot be rebound",
            ));
        }
        let directories = match held_directories {
            Some(directories) => directories,
            None => reopen_directories(&engine, intent)?,
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
        progress(AdditionPhase::Enrolling, Some(binding.clone()))?;
        if engine.pending_membership_status().is_some() {
            engine.resume_pending_members().map_err(error)?;
        } else {
            let directories = reopen_directories(&engine, intent)?;
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
            Some(&binding),
        )
    }
}

fn cancel_addition(
    engine: &mut AdmissionEngine,
    identity: &Identity,
    intent: &StoredAddition,
    progress: &mut impl FnMut(AdditionPhase, Option<ArchiveBinding>) -> Result<(), ApiError>,
) -> Result<OperationResult, ApiError> {
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
    // Reuse complete F without requiring every old member to become available.
    // Only constructing F requires the original full fence/journal closure.
    let binding = match &intent.archive {
        Some(binding) => binding.clone(),
        None => {
            progress(AdditionPhase::Fencing, None)?;
            engine.resume_membership_fence().map_err(error)?;
            current_protected_identity(engine, identity)?;
            progress(AdditionPhase::Archiving, None)?;
            let archive = engine
                .build_managed_membership_control_archive()
                .map_err(error)?;
            ArchiveBinding {
                archive_path: archive.archive_path,
                root_hash: archive.root_hash,
                transition_hash: archive.transition_hash,
            }
        }
    };
    // This durable continuation is retained even when selection fails part-way.
    // No error automatically authorizes returning to the Add path.
    progress(AdditionPhase::CertifyingAbort, Some(binding.clone()))?;
    current_protected_identity(engine, identity)?;
    let certified = engine
        .certify_membership_abort(&binding.archive_path, &binding.root_hash)
        .map_err(error)?;
    if certified.operation_id != operation
        || certified.transition_hash != binding.transition_hash
        || certified.original_members != intent.status.preview.original_metadata_members
        || certified.original_quorum != certified.original_members / 2 + 1
        || certified.confirmed_members < certified.original_quorum
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "Abort certificate did not confirm this original addition and fixed old quorum",
        ));
    }
    progress(AdditionPhase::AbortCertified, None)?;
    current_protected_identity(engine, identity)?;
    progress(AdditionPhase::Cancelling, None)?;
    let result = engine
        .cancel_membership_handoff(&binding.archive_path, &binding.root_hash)
        .map_err(error)?;
    if result.kind != "membership-add-cancelled" {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "certified Abort did not publish the original cancellation result",
        ));
    }
    exact_result(result, operation, Some(&binding))
}

fn published_result(
    engine: &Engine,
    identity: &Identity,
    intent: &StoredAddition,
) -> Result<Option<OperationResult>, ApiError> {
    // Unpublished lookup can scan unavailable original journals. That must not
    // impose an all-old-members requirement before fixed-old-Q cancellation.
    if !engine
        .state()
        .operations
        .contains_key(&intent.status.operation_id)
    {
        return Ok(None);
    }
    let record = match engine.operation_record(intent.status.operation_id) {
        Ok(record) => record,
        Err(source) if source.code() == storage_contract::ErrorCode::OperationNotFound => {
            return Ok(None);
        }
        Err(source) => return Err(error(source)),
    };
    if record.owner() != Some(identity.principal.as_str()) {
        return Err(ApiError::forbidden());
    }
    if record.result().commit_state != storage_contract::CommitState::Published {
        return Ok(None);
    }
    if intent.status.preview.authority.is_terminal() {
        crate::management::terminal_resume::exact_result(record.into_result(), intent).map(Some)
    } else {
        exact_result(
            record.into_result(),
            intent.status.operation_id,
            intent.archive.as_ref(),
        )
        .map(Some)
    }
}

fn exact_result(
    result: OperationResult,
    operation: storage_contract::OperationId,
    archive: Option<&ArchiveBinding>,
) -> Result<OperationResult, ApiError> {
    if result.operation_id != operation
        || !matches!(
            result.kind.as_str(),
            "membership-add" | "membership-add-cancelled"
        )
        || result.commit_state != storage_contract::CommitState::Published
        || archive.is_none_or(|binding| result.request_digest != binding.transition_hash)
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "membership publication did not confirm the original addition",
        ));
    }
    Ok(result)
}

fn reopen_directories(
    engine: &Engine,
    intent: &StoredAddition,
) -> Result<Vec<storage_engine::PendingMemberDirectory>, ApiError> {
    let stopped = stopped_roots(engine);
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
                directory: crate::storage_directories::reopen_fresh_addition(
                    &addition.selection,
                    &stopped,
                )?,
            })
        })
        .collect()
}

fn stopped_roots(engine: &Engine) -> Vec<std::path::PathBuf> {
    let enabled: std::collections::BTreeSet<_> = engine
        .registered_disks()
        .filter(|disk| disk.registration_mode == storage_engine::RegistrationMode::Enabled)
        .map(|disk| disk.disk_id)
        .collect();
    engine
        .config()
        .disks
        .iter()
        .filter(|disk| !enabled.contains(disk.disk_id.as_str()))
        .map(|disk| disk.root.clone())
        .collect()
}
