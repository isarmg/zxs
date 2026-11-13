//! Retained-directory admission consumes the authenticated terminal owner. The
//! preview and routing checks read SYSTEM metadata only; only an explicit commit
//! or continuation may supply the exact registered directory to the core.
use super::*;
use platform_io::fs_safety::PrivateDirectory;
use storage_addition::{ArchiveBinding, configuration_digest, digest, latest_name, write};
use storage_contract::{Config, ErrorCode, storage_readmission::*};
use storage_engine::{
    TerminalManagement, TerminalMetadataResume, TerminalMetadataResumeAdmission,
    TerminalMetadataResumePhase, TerminalMetadataResumeRecovery, TerminalReadmissionPreview,
};
use storage_readmission::{StoredReadmission, finish, save};

fn recovery(message: &'static str) -> ApiError {
    ApiError::conflict(ErrorCode::RecoveryRequired, message)
}
fn different_owner() -> ApiError {
    ApiError::conflict(
        ErrorCode::IdempotencyKeyReused,
        "terminal readmission does not own this admission kind, actor, operation or registration",
    )
}

pub(super) fn preview(
    terminal: &TerminalManagement,
    principal: &str,
    request: ReadmissionRequest,
) -> Result<(ReadmissionPreview, Config), ApiError> {
    if terminal.operation(request.operation_id).is_some() {
        return Err(ApiError::conflict(
            ErrorCode::IdempotencyKeyReused,
            "this operation_id already belongs to permanent history",
        ));
    }
    let source = terminal
        .preview_disk_readmission(&request.expected_disk_id)
        .map_err(crate::engine_backend::error)?;
    let original = (**terminal.original_configuration()).clone();
    let preview = project_preview(
        source,
        &original,
        request,
        principal,
        terminal.status().operation_id,
    )?;
    Ok((preview, original))
}

fn project_preview(
    source: TerminalReadmissionPreview,
    original: &Config,
    request: ReadmissionRequest,
    principal: &str,
    terminal_operation_id: storage_contract::OperationId,
) -> Result<ReadmissionPreview, ApiError> {
    if source.target.slot_id != request.slot_id {
        return Err(ApiError::conflict(
            ErrorCode::GenerationMismatch,
            "the selected slot is not this retained registration",
        ));
    }
    let mut preview = ReadmissionPreview {
        request,
        target: ReadmissionTarget {
            pool_id: source.target.pool_id,
            disk_id: source.target.disk_id,
            slot_id: source.target.slot_id,
            binding_generation: source.target.binding_generation,
            root_path: source
                .root
                .to_str()
                .ok_or_else(|| ApiError::invalid("registered directory is not a UTF-8 path"))?
                .to_owned(),
        },
        principal: principal.to_owned(),
        metadata_member: source.metadata_member,
        configuration_digest: configuration_digest(original),
        preview_digest: String::new(),
        authority: ReadmissionAuthority::TerminalManagement {
            terminal_authority_hash: source.terminal_authority_hash,
            terminal_operation_id,
        },
        registry_root_hash: source.registry.root_hash().to_owned(),
        registry_generation: source.registry.view_generation(),
        source_sequence: source.source_sequence,
        topology_generation: source.topology_generation,
        snapshot_hash: source.snapshot_hash,
    };
    preview.preview_digest = digest(&preview);
    Ok(preview)
}

pub(super) fn require_preview(
    terminal: &TerminalManagement,
    principal: &str,
    intent: &StoredReadmission,
) -> Result<(), ApiError> {
    let (current, _) = preview(terminal, principal, intent.status.preview.request.clone())?;
    if current != intent.status.preview {
        return Err(ApiError::conflict(
            ErrorCode::GenerationMismatch,
            "terminal authority, registration or source boundary changed since this preview",
        ));
    }
    Ok(())
}

fn require_admission(
    admission: &TerminalMetadataResumeAdmission,
    intent: &StoredReadmission,
) -> Result<(), ApiError> {
    let TerminalMetadataResumeAdmission::RetainedMetadata { target } = admission else {
        return Err(different_owner());
    };
    let expected = &intent.status.preview.target;
    if !intent.status.preview.authority.is_terminal()
        || !intent.status.preview.metadata_member
        || target.pool_id != expected.pool_id
        || target.disk_id != expected.disk_id
        || target.slot_id != expected.slot_id
        || target.binding_generation != expected.binding_generation
    {
        return Err(different_owner());
    }
    Ok(())
}

pub(super) fn bind_archive(
    intent: &mut StoredReadmission,
    binding: ArchiveBinding,
) -> Result<(), ApiError> {
    if intent
        .archive
        .as_ref()
        .is_some_and(|saved| saved != &binding)
    {
        return Err(recovery(
            "terminal readmission archive differs from the original operation",
        ));
    }
    intent.archive = Some(binding);
    Ok(())
}

pub(super) fn pending_binding(
    pending: &TerminalMetadataResume,
    intent: &StoredReadmission,
) -> Result<ArchiveBinding, ApiError> {
    // These accessors do no I/O. Reject a fresh Add or another actor before
    // moving the live SYSTEM owner out of BackendState.
    require_admission(&pending.admission(), intent)?;
    if pending.operation_id() != intent.status.operation_id
        || pending.principal() != intent.principal
    {
        return Err(different_owner());
    }
    require_preview(pending.terminal_management(), pending.principal(), intent)?;
    let status = pending.status().map_err(crate::engine_backend::error)?;
    if status.terminal_authority_hash != intent.status.preview.authority.hash()
        || status.target_metadata_members != 1
        || status.target_metadata_quorum != 1
    {
        return Err(different_owner());
    }
    let archive = pending.control_archive();
    let binding = ArchiveBinding {
        archive_path: archive.archive_path,
        root_hash: archive.root_hash,
        transition_hash: status.transition_hash,
    };
    if intent
        .archive
        .as_ref()
        .is_some_and(|saved| saved != &binding)
    {
        return Err(different_owner());
    }
    Ok(binding)
}

pub(super) fn recovery_binding(
    proof: &TerminalMetadataResumeRecovery,
    intent: &StoredReadmission,
) -> Result<ArchiveBinding, ApiError> {
    require_admission(&proof.admission, intent)?;
    if proof.operation_id != intent.status.operation_id
        || proof.principal != intent.principal
        || proof.terminal_authority_hash != intent.status.preview.authority.hash()
        || configuration_digest(&proof.target) != configuration_digest(&intent.target_config())
    {
        return Err(different_owner());
    }
    let current = project_preview(
        proof
            .readmission_preview
            .clone()
            .ok_or_else(different_owner)?,
        &proof.original,
        intent.status.preview.request.clone(),
        &proof.principal,
        proof.terminal_operation_id,
    )?;
    if current != intent.status.preview {
        return Err(different_owner());
    }
    let binding = ArchiveBinding {
        archive_path: proof.control_archive.archive_path.clone(),
        root_hash: proof.control_archive.root_hash.clone(),
        transition_hash: proof.transition_hash.clone(),
    };
    if intent
        .archive
        .as_ref()
        .is_some_and(|saved| saved != &binding)
    {
        return Err(different_owner());
    }
    Ok(binding)
}

enum ResumeOwner {
    Pending(Box<TerminalMetadataResume>),
    Published(Box<storage_engine::Engine>),
}

impl Inner {
    fn verify_terminal_readmission_owner(
        &self,
        identity: &Identity,
        intent: &StoredReadmission,
    ) -> Result<(), ApiError> {
        self.current_management_identity(identity)?;
        {
            let state = crate::lock(&self.state);
            match &state.backend {
                BackendState::Terminal(terminal) => {
                    return require_preview(terminal, &identity.principal, intent);
                }
                BackendState::MetadataResume(pending) => {
                    pending_binding(pending, intent)?;
                    return Ok(());
                }
                BackendState::Engine(_) => return Ok(()),
                BackendState::ResumeRecovery | BackendState::Unopened => {}
                BackendState::Draining(_) | BackendState::ChangingAuthority => {
                    return Err(recovery(
                        "another authority handover owns the storage backend",
                    ));
                }
            }
        }
        // Only cold owners use this SYSTEM discriminator. Opening SYSTEM while
        // a live pending owner holds its writer would violate exclusivity.
        if let Some(proof) = TerminalManagement::metadata_resume_recovery(
            self.storage_system()?.path(),
        )
            .map_err(crate::engine_backend::error)?
        {
            recovery_binding(&proof, intent)?;
            return Ok(());
        }
        let terminal =
            TerminalManagement::open(self.storage_system()?.path())
                    .map_err(crate::engine_backend::error)?;
        require_preview(&terminal, &identity.principal, intent)
    }

    fn restore_readmission(&self, pending: TerminalMetadataResume) {
        let mut state = crate::lock(&self.state);
        state.backend = BackendState::MetadataResume(Box::new(pending));
        state.admitted = false;
        state.status = StorageState::RecoveryRequired;
    }

    fn prepare_terminal_readmission(
        &self,
        identity: &Identity,
        intent: &mut StoredReadmission,
        terminal: TerminalManagement,
        directory: &PrivateDirectory,
    ) -> Result<TerminalMetadataResume, ApiError> {
        let ready = (|| {
            require_preview(&terminal, &identity.principal, intent)?;
            self.current_management_identity(identity)?;
            intent.status.phase = ReadmissionPhase::Fencing;
            save(directory, intent)?;
            crate::engine_backend::storage_readmission::open_target(intent)
        })();
        let held = match ready {
            Ok(held) => held,
            Err(error) => {
                crate::lock(&self.state).backend = BackendState::Terminal(Arc::new(terminal));
                return Err(error);
            }
        };
        let auth = crate::lock(&self.auth)
            .clone()
            .expect("invariant: current management identity has an auth owner");
        let current = crate::lock(&auth);
        if let Err(error) = self
            .ensure_running()
            .and_then(|()| current.revalidate_identity(identity).map(|_| ()))
        {
            drop(current);
            crate::lock(&self.state).backend = BackendState::Terminal(Arc::new(terminal));
            return Err(error);
        }
        let pending = terminal
            .prepare_disk_readmission(
                &intent.status.preview.target.disk_id,
                &identity.principal,
                intent.status.operation_id,
                held,
            )
            .map_err(crate::engine_backend::error);
        drop(current);
        pending
    }

    fn reopen_terminal_readmission(
        &self,
        identity: &Identity,
        intent: &mut StoredReadmission,
        directory: &PrivateDirectory,
    ) -> Result<ResumeOwner, ApiError> {
        // Keep the admitted SYSTEM owner across proof lookup and authorized reopen.
        let system = self.storage_system()?;
        let proof = TerminalManagement::metadata_resume_recovery(
            system.path(),
        )
            .map_err(crate::engine_backend::error)?;
        let Some(proof) = proof else {
            let terminal =
                TerminalManagement::open(system.path())
                    .map_err(crate::engine_backend::error)?;
            return self
                .prepare_terminal_readmission(identity, intent, terminal, directory)
                .map(|pending| ResumeOwner::Pending(Box::new(pending)));
        };
        let binding = recovery_binding(&proof, intent)?;
        bind_archive(intent, binding)?;
        save(directory, intent)?;
        self.current_management_identity(identity)?;
        let management = self.with_terminal_authorization(identity, || {
            TerminalManagement::open_metadata_resume_checked(
                        system.path(), &proof,
                    )
        })?;
        match proof.phase {
            TerminalMetadataResumePhase::ResupplyDirectories => {
                let held = crate::engine_backend::storage_readmission::open_target(intent)?;
                self.with_terminal_authorization(identity, || {
                    management.reopen_metadata_resume_held(vec![held])
                })
                .map(|pending| ResumeOwner::Pending(Box::new(pending)))
            }
            TerminalMetadataResumePhase::CompleteGeneration => self
                .with_terminal_authorization(identity, || {
                    management.complete_metadata_resume_held(self.retained_workspace_limits)
                })
                .map(|engine| ResumeOwner::Published(Box::new(engine))),
        }
    }

    fn run_terminal_readmission(
        self: &Arc<Self>,
        identity: &Identity,
        intent: &mut StoredReadmission,
        directory: &PrivateDirectory,
    ) -> Result<(), ApiError> {
        if crate::lock(&self.state).backend.active_engine().is_some() {
            return self.recover_once();
        }
        let previous = {
            let mut state = crate::lock(&self.state);
            if let BackendState::MetadataResume(pending) = &state.backend {
                pending_binding(pending, intent)?;
            }
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
            std::mem::replace(&mut state.backend, BackendState::ChangingAuthority)
        };
        let owner = match previous {
            BackendState::Terminal(terminal) => match Arc::try_unwrap(terminal) {
                Ok(terminal) => self
                    .prepare_terminal_readmission(identity, intent, terminal, directory)
                    .map(|pending| ResumeOwner::Pending(Box::new(pending))),
                Err(terminal) => {
                    crate::lock(&self.state).backend = BackendState::Terminal(terminal);
                    Err(ApiError::conflict(
                        ErrorCode::StorageDraining,
                        "management readers must finish before this readmission can take its authority",
                    ))
                }
            },
            BackendState::MetadataResume(pending) => Ok(ResumeOwner::Pending(pending)),
            BackendState::ResumeRecovery | BackendState::Unopened => {
                self.reopen_terminal_readmission(identity, intent, directory)
            }
            other => {
                crate::lock(&self.state).backend = other;
                Err(recovery(
                    "another authority handover owns the storage backend",
                ))
            }
        };
        let owner = match owner {
            Ok(owner) => owner,
            Err(error) => {
                let mut state = crate::lock(&self.state);
                if matches!(state.backend, BackendState::ChangingAuthority) {
                    state.backend = BackendState::ResumeRecovery;
                }
                return Err(error);
            }
        };
        let engine = match owner {
            ResumeOwner::Published(engine) => *engine,
            ResumeOwner::Pending(pending) => {
                self.activate_terminal_readmission(identity, intent, *pending, directory)?
            }
        };
        {
            let mut state = crate::lock(&self.state);
            state.backend = BackendState::Engine(Arc::new(EngineBackend::new(engine)));
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
        }
        crate::lock(&self.disk_observation).take();
        // Current SYSTEM credentials, rather than archived credentials, admit
        // the returned engine through the same auth-replication path as startup.
        self.recover_once()
    }

    fn activate_terminal_readmission(
        &self,
        identity: &Identity,
        intent: &mut StoredReadmission,
        mut pending: TerminalMetadataResume,
        directory: &PrivateDirectory,
    ) -> Result<storage_engine::Engine, ApiError> {
        let prepared = (|| {
            self.current_management_identity(identity)?;
            let binding = pending_binding(&pending, intent)?;
            bind_archive(intent, binding)?;
            intent.status.phase = ReadmissionPhase::Archiving;
            save(directory, intent)?;
            self.with_terminal_authorization(identity, || pending.synchronize())?;
            self.current_management_identity(identity)?;
            intent.status.phase = ReadmissionPhase::Activating;
            save(directory, intent)
        })();
        if let Err(error) = prepared {
            self.restore_readmission(pending);
            return Err(error);
        }
        let auth = crate::lock(&self.auth)
            .clone()
            .expect("invariant: prepared readmission has management auth");
        let current = crate::lock(&auth);
        if let Err(error) = self
            .ensure_running()
            .and_then(|()| current.revalidate_identity(identity).map(|_| ()))
        {
            drop(current);
            self.restore_readmission(pending);
            return Err(error);
        }
        let engine = pending
            .activate_with_workspace_limits(self.retained_workspace_limits)
            .map_err(crate::engine_backend::error);
        drop(current);
        if engine.is_err() {
            crate::lock(&self.state).backend = BackendState::ResumeRecovery;
        }
        engine
    }
}

impl ManagementBackend {
    pub(super) fn execute_terminal_readmission(
        &self,
        identity: &Identity,
        directory: &PrivateDirectory,
        mut intent: StoredReadmission,
    ) -> Result<Value, ApiError> {
        if self.inner.recovering.swap(true, Ordering::SeqCst) {
            return Err(ApiError::conflict(
                ErrorCode::StorageRetryRunning,
                "storage recovery is already running",
            ));
        }
        let _recovery = RecoveryPermit(&self.inner.recovering);
        self.inner
            .verify_terminal_readmission_owner(identity, &intent)?;
        intent.status.state = ReadmissionState::Running;
        intent.status.failure = None;
        save(directory, &intent)?;
        write(directory, &latest_name(), &intent.status.operation_id)?;
        let outcome = self
            .inner
            .run_terminal_readmission(identity, &mut intent, directory);
        // A failed auth-replica or HTTP save can follow core publication. Always
        // look up that permanent receipt before reporting an uncertain outcome.
        if let Some(result) = self.readmission_result(identity, &intent)? {
            finish(&mut intent, result);
            save(directory, &intent)?;
            return Ok(json_codec::to_value(intent.status)
                .expect("invariant: readmission status serializes"));
        }
        let error = match outcome {
            Ok(()) => recovery("resumed engine has no matching permanent readmission receipt"),
            Err(error) => error,
        };
        intent.status.state = ReadmissionState::RecoveryRequired;
        intent.status.failure = Some(ReadmissionFailure {
            code: error.code,
            message: error.message.clone(),
        });
        if let Err(persistence) = save(directory, &intent) {
            tracing::error!(error = ?error, persistence = ?persistence,
                "terminal readmission failed and its diagnostic could not be synchronized");
            return Err(persistence);
        }
        Err(ApiError::new(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::RecoveryRequired,
            format!(
                "readmission {} needs recovery; continue the original operation",
                intent.status.operation_id
            ),
        )
        .with_source(Arc::new(error)))
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/terminal_readmission.rs"]
mod tests;
