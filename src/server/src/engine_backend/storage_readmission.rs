//! The explicit commit is the only path that opens a Disabled target. Previews
//! and receipt reads borrow authenticated registry metadata without probing it.
use super::*;
use crate::management::{
    storage_addition::{ArchiveBinding, configuration_digest, digest},
    storage_readmission::StoredReadmission,
};
use platform_io::fs_safety::{
    PrivateDirectory,
    linux::{MountPolicy, OpenAt2Root},
};
use storage_contract::{Config, ErrorCode, OperationResult, storage_readmission::*};
use storage_engine::{
    DiskReadmissionPhase, PendingMemberDirectory, RegistrationMode as EngineRegistrationMode,
};

impl EngineBackend {
    pub(crate) fn storage_registration_snapshot(
        &self,
        identity: &Identity,
    ) -> Result<RegistrationSnapshot, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        if engine.registered_disks().next().is_none() {
            return Ok(RegistrationSnapshot::Unadmitted);
        }
        Ok(RegistrationSnapshot::ActiveMembership {
            registrations: engine
                .registered_disks()
                .map(|disk| {
                    let mut entry = registration_entry(disk);
                    if entry.mode == RegistrationMode::Disabled {
                        entry.readmission = Some(
                            if engine
                                .preview_disk_readmission_metadata_member(&entry.disk_id)
                                .map_err(error)?
                            {
                                ReadmissionRole::MetadataMember
                            } else {
                                ReadmissionRole::DataOnly
                            },
                        );
                    }
                    Ok(entry)
                })
                .collect::<Result<Vec<_>, ApiError>>()?,
        })
    }

    pub(crate) fn preview_storage_readmission(
        &self,
        identity: &Identity,
        request: ReadmissionRequest,
    ) -> Result<(ReadmissionPreview, Config), ApiError> {
        let (engine, actor) = self.lock_current(identity)?;
        preview(&engine, &actor.principal, request)
    }

    pub(crate) fn storage_readmission_result(
        &self,
        identity: &Identity,
        intent: &StoredReadmission,
    ) -> Result<Option<OperationResult>, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        published_result(&engine, identity, intent)
    }

    pub(crate) fn run_storage_readmission(
        &self,
        identity: &Identity,
        intent: &StoredReadmission,
        mut progress: impl FnMut(ReadmissionPhase, Option<ArchiveBinding>) -> Result<(), ApiError>,
    ) -> Result<OperationResult, ApiError> {
        if intent.status.preview.authority.is_terminal() {
            return Err(ApiError::conflict(
                ErrorCode::IdempotencyKeyReused,
                "terminal readmission cannot be driven as an active membership operation",
            ));
        }
        let (mut engine, actor) = self.lock_current(identity)?;
        if let Some(result) = published_result(&engine, identity, intent)? {
            return Ok(result);
        }
        let operation = intent.status.operation_id;
        let existing = engine.disk_readmission_status().map_err(error)?;
        let existing = match existing {
            Some(status) if status.operation_id == operation => {
                verify_status(&status, intent)?;
                Some(status)
            }
            Some(status) if status.phase != DiskReadmissionPhase::Published => {
                return Err(ApiError::conflict(
                    ErrorCode::IdempotencyKeyReused,
                    "a different readmission still owns this membership authority",
                ));
            }
            _ => None,
        };
        if existing.is_none() {
            let (current, _) = preview(
                &engine,
                &actor.principal,
                intent.status.preview.request.clone(),
            )?;
            if current != intent.status.preview {
                return Err(ApiError::conflict(
                    ErrorCode::GenerationMismatch,
                    "readmission preview no longer matches the current Disabled registration and authority",
                ));
            }
        }
        // The caller explicitly submitted this exact preview (or continued its
        // durable operation). No discovery, page polling or readiness check can
        // reach this open. Core checks the held root against the original seal.
        let directory = open_target(intent)?;
        current_protected_identity(&engine, identity)?;
        if existing.as_ref().is_none_or(|status| {
            !matches!(
                status.phase,
                DiskReadmissionPhase::Activating | DiskReadmissionPhase::Published
            )
        }) {
            let begin_directory = PendingMemberDirectory {
                disk_id: directory.disk_id.clone(),
                directory: directory.directory.try_clone().map_err(ApiError::io)?,
            };
            // Persist the intent only after the engine-locked CAS above succeeds.
            progress(ReadmissionPhase::Fencing, None)?;
            let status = engine
                .begin_disk_readmission(
                    &intent.status.preview.target.disk_id,
                    &actor.principal,
                    operation,
                    begin_directory,
                )
                .map_err(error)?;
            verify_status(&status, intent)?;
        }
        current_protected_identity(&engine, identity)?;
        let binding = match &intent.archive {
            Some(binding) => binding.clone(),
            None => {
                progress(ReadmissionPhase::Archiving, None)?;
                let archive = engine
                    .build_managed_membership_control_archive()
                    .map_err(error)?;
                let binding = ArchiveBinding {
                    archive_path: archive.archive_path,
                    root_hash: archive.root_hash,
                    transition_hash: archive.transition_hash,
                };
                let status = engine
                    .disk_readmission_status()
                    .map_err(error)?
                    .ok_or_else(|| {
                        ApiError::conflict(
                            ErrorCode::RecoveryRequired,
                            "readmission fence is missing",
                        )
                    })?;
                verify_status(&status, intent)?;
                if binding.transition_hash != status.transition_hash {
                    return Err(ApiError::conflict(
                        ErrorCode::RecoveryRequired,
                        "readmission archive differs from its original fence",
                    ));
                }
                binding
            }
        };
        current_protected_identity(&engine, identity)?;
        progress(ReadmissionPhase::Activating, Some(binding.clone()))?;
        current_protected_identity(&engine, identity)?;
        let result = engine
            .activate_disk_readmission(&binding.archive_path, &binding.root_hash, directory)
            .map_err(error)?;
        exact_result(result, intent, Some(&binding))
    }
}

fn preview(
    engine: &Engine,
    principal: &str,
    request: ReadmissionRequest,
) -> Result<(ReadmissionPreview, Config), ApiError> {
    if engine.membership_fence_status().is_some() {
        return Err(ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "continue the original membership operation before previewing readmission",
        ));
    }
    if engine
        .state()
        .operations
        .contains_key(&request.operation_id)
    {
        return Err(ApiError::conflict(
            ErrorCode::IdempotencyKeyReused,
            "this operation_id already belongs to a published operation",
        ));
    }
    let registered = engine
        .registered_disk(&request.expected_disk_id)
        .filter(|disk| disk.slot_id == request.slot_id)
        .ok_or_else(|| {
            ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "the selected directory is no longer this current registration",
            )
        })?;
    if registered.registration_mode != EngineRegistrationMode::Disabled {
        return Err(ApiError::conflict(
            ErrorCode::GenerationMismatch,
            "readmission requires a currently Disabled registration",
        ));
    }
    let registry = engine.current_disk_registry_commitment().ok_or_else(|| {
        ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "readmission requires an admitted registry",
        )
    })?;
    let identity = registered.identity();
    let projection = engine
        .preview_disk_deregistration(&identity)
        .map_err(|source| {
            ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "readmission source boundary is unavailable",
            )
            .with_source(source)
        })?;
    let boundary = projection.boundary();
    let authority_hash = boundary
        .admitted_membership_generation_hash
        .ok_or_else(|| {
            ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "readmission requires an authenticated membership authority",
            )
        })?;
    let snapshot_hash = boundary.published_snapshot_hash.ok_or_else(|| {
        ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "readmission requires a published source boundary",
        )
    })?;
    let config = engine.config().clone();
    let disk = config
        .disks
        .iter()
        .find(|disk| disk.disk_id == identity.disk_id)
        .expect("invariant: current registration retains its historical definition");
    let metadata_member = engine
        .preview_disk_readmission_metadata_member(&identity.disk_id)
        .map_err(error)?;
    let mut preview = ReadmissionPreview {
        request,
        target: ReadmissionTarget {
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
        principal: principal.to_owned(),
        metadata_member,
        configuration_digest: configuration_digest(&config),
        preview_digest: String::new(),
        authority: ReadmissionAuthority::ActiveMembership {
            membership_authority_hash: authority_hash.to_owned(),
        },
        registry_root_hash: registry.root_hash().to_owned(),
        registry_generation: registry.view_generation(),
        source_sequence: boundary.sequence,
        topology_generation: boundary.topology_generation,
        snapshot_hash: snapshot_hash.to_owned(),
    };
    preview.preview_digest = digest(&preview);
    Ok((preview, config))
}

pub(crate) fn open_target(intent: &StoredReadmission) -> Result<PendingMemberDirectory, ApiError> {
    let target = &intent.status.preview.target;
    let root = PrivateDirectory::open_existing(&target.root_path).map_err(directory_error)?;
    let root = OpenAt2Root::from_private_directory(root, MountPolicy::SameMount)
        .map_err(directory_error)?;
    Ok(PendingMemberDirectory {
        disk_id: target.disk_id.clone(),
        directory: root.into_directory(),
    })
}

fn directory_error(source: platform_io::fs_safety::Error) -> ApiError {
    ApiError::new(
        http::StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::DiskUnavailable,
        "the explicitly selected Disabled directory could not be opened safely",
    )
    .with_source(source)
}

fn published_result(
    engine: &Engine,
    identity: &Identity,
    intent: &StoredReadmission,
) -> Result<Option<OperationResult>, ApiError> {
    let operation = intent.status.operation_id;
    if engine.config().pool_id != intent.status.preview.target.pool_id {
        return Err(ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "readmission belongs to another pool",
        ));
    }
    if let Some(status) = engine.disk_readmission_status().map_err(error)?
        && status.operation_id == operation
    {
        verify_status(&status, intent)?;
        if status.phase != DiskReadmissionPhase::Published {
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

pub(crate) fn verify_status(
    status: &storage_engine::DiskReadmissionStatus,
    intent: &StoredReadmission,
) -> Result<(), ApiError> {
    let preview = &intent.status.preview;
    let target = &preview.target;
    let source_authority_matches = matches!(
        &preview.authority,
        ReadmissionAuthority::ActiveMembership { membership_authority_hash }
            if membership_authority_hash == &status.source_membership_authority_hash
    );
    if status.operation_id != intent.status.operation_id
        || status.principal != intent.status.preview.principal
        || status.target.pool_id != target.pool_id
        || status.target.disk_id != target.disk_id
        || status.target.slot_id != target.slot_id
        || status.target.binding_generation != target.binding_generation
        || status.metadata_member != preview.metadata_member
        || !source_authority_matches
        || status.source_configuration_digest != preview.configuration_digest
        || status.source_configuration_digest != configuration_digest(&intent.original)
        || status.source_root_path != std::path::Path::new(&target.root_path)
        || status.source_registry_root_hash != preview.registry_root_hash
        || status.source_registry_generation != preview.registry_generation
        || status.source_sequence != preview.source_sequence
        || status.source_topology_generation != preview.topology_generation
        || status.source_snapshot_hash != preview.snapshot_hash
        || intent
            .archive
            .as_ref()
            .is_some_and(|archive| archive.transition_hash != status.transition_hash)
    {
        return Err(ApiError::conflict(
            ErrorCode::IdempotencyKeyReused,
            "core readmission differs from the original actor, operation, registration or source",
        ));
    }
    Ok(())
}

pub(crate) fn exact_result(
    result: OperationResult,
    intent: &StoredReadmission,
    archive: Option<&ArchiveBinding>,
) -> Result<OperationResult, ApiError> {
    let target = &intent.status.preview.target;
    let binding_generation = target.binding_generation.to_string();
    let outcome = |key: &str| result.result.get(key).and_then(json_codec::Value::as_str);
    let authority_matches = match &intent.status.preview.authority {
        ReadmissionAuthority::ActiveMembership { .. } => {
            result.result.get("authority_kind").is_none()
        }
        ReadmissionAuthority::TerminalManagement {
            terminal_authority_hash,
            ..
        } => {
            outcome("authority_kind") == Some("terminal-metadata-resume")
                && outcome("terminal_authority") == Some(terminal_authority_hash.as_str())
        }
    };
    if result.operation_id != intent.status.operation_id
        || !authority_matches
        || result.kind != "disk-readmit"
        || result.commit_state != storage_contract::CommitState::Published
        || outcome("disk_id") != Some(target.disk_id.as_str())
        || outcome("slot_id") != Some(target.slot_id.as_str())
        || outcome("binding_generation") != Some(binding_generation.as_str())
        || outcome("registration") != Some("enabled")
        || outcome("io_admission_epoch") != Some(result.request_digest.as_str())
        || archive.is_none_or(|archive| result.request_digest != archive.transition_hash)
    {
        return Err(ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "readmission did not confirm the original identity and new I/O epoch publication",
        ));
    }
    Ok(result)
}

pub(crate) fn registration_entry(disk: storage_engine::RegisteredDisk<'_>) -> RegistrationEntry {
    RegistrationEntry {
        slot_id: disk.slot_id.to_owned(),
        disk_id: disk.disk_id.to_owned(),
        binding_generation: disk.binding_generation,
        mode: match disk.registration_mode {
            EngineRegistrationMode::Enabled => RegistrationMode::Enabled,
            EngineRegistrationMode::Disabled => RegistrationMode::Disabled,
        },
        metadata_member: disk.metadata_member,
        readmission: None,
    }
}
