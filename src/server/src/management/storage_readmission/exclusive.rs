//! Synchronous native owner for an exact retained terminal metadata admission.
//! No service threads or implicit Add. The caller owns the host lifetime lease.
use super::super::terminal_readmission as terminal;
use super::*;
use crate::auth::AuthReader;
use platform_io::fs_safety::AdvisoryLock;
use storage_engine::{
    Engine, Mode, TerminalManagement, TerminalMetadataResume, TerminalMetadataResumePhase,
};

/// Primary AuthStore and host exclusion must outlive this owner. Shared native
/// records/results are the same ones used by the service, not helper-only flags.
pub struct ExclusiveTerminalReadmission {
    control: PrivateDirectory,
    system: PrivateDirectory,
    management: PrivateDirectory,
    auth: AuthReader,
    identity: Identity,
    _owner: AdvisoryLock,
}
impl ExclusiveTerminalReadmission {
    pub fn open(
        control: PrivateDirectory,
        system: PrivateDirectory,
        management: PrivateDirectory,
        auth: AuthReader,
        identity: Identity,
    ) -> Result<Self, ApiError> {
        auth.revalidate_identity(&identity)?;
        if identity.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        control.verify_identity().map_err(storage_error)?;
        system.verify_identity().map_err(storage_error)?;
        management.verify_identity().map_err(storage_error)?;
        let owner = AdvisoryLock::acquire(
            &management,
            &EntryName::new("storage-addition-owner.lock")
                .expect("invariant: fixed lifecycle owner")
                .as_relative(),
        )
        .map_err(storage_error)?;
        Ok(Self {
            control,
            system,
            management,
            auth,
            identity,
            _owner: owner,
        })
    }
    fn check(&self) -> Result<(), ApiError> {
        self.auth.revalidate_identity(&self.identity)?;
        self.control.verify_identity().map_err(storage_error)?;
        self.system.verify_identity().map_err(storage_error)?;
        self.management.verify_identity().map_err(storage_error)
    }
    fn settled_other(&self) -> Result<(), ApiError> {
        self.check()?;
        super::super::storage_addition::require_no_pending_addition(self.management.path())?;
        super::super::storage_lifecycle::require_settled_exit(self.management.path())
    }
    fn actor(&self, record: &StoredReadmission) -> Result<(), ApiError> {
        self.check()?;
        if record.principal != self.identity.principal {
            return Err(ApiError::forbidden());
        }
        if !record.status.preview.authority.is_terminal() || !record.status.preview.metadata_member
        {
            return Err(recovery(
                "this owner only continues an exact retained terminal metadata admission",
            ));
        }
        if record.original.mode != Mode::Production {
            return Err(recovery(
                "offline terminal readmission requires an existing production pool",
            ));
        }
        Ok(())
    }
    fn prepare(
        &self,
        request: ReadmissionRequest,
    ) -> Result<(ReadmissionPreview, Config), ApiError> {
        self.settled_other()?;
        require_no_pending_readmission(self.management.path())?;
        if !TerminalManagement::requires_management(self.system.path())
            .map_err(crate::engine_backend::error)?
        {
            return Err(recovery(
                "readmission preview requires current terminal authority, not an active or empty pool",
            ));
        }
        let terminal =
            TerminalManagement::open(self.system.path()).map_err(crate::engine_backend::error)?;
        if terminal.original_configuration().mode != Mode::Production {
            return Err(recovery(
                "offline terminal readmission requires an existing production pool",
            ));
        }
        // Core refuses Detached identities, nonmetadata targets, unfinished
        // retirement and completed retirement chains before any target I/O.
        terminal::preview(&terminal, &self.identity.principal, request)
    }
    pub fn preview(&self, request: ReadmissionRequest) -> Result<ReadmissionPreview, ApiError> {
        let preview = self.prepare(request)?.0;
        self.check()?;
        Ok(preview)
    }
    fn query_result(
        &self,
        record: &mut StoredReadmission,
    ) -> Result<Option<OperationResult>, ApiError> {
        self.actor(record)?;
        if TerminalManagement::requires_management(self.system.path())
            .map_err(crate::engine_backend::error)?
        {
            if let Some(proof) = TerminalManagement::metadata_resume_recovery(self.system.path())
                .map_err(crate::engine_backend::error)?
            {
                let binding = terminal::recovery_binding(&proof, record)?;
                terminal::bind_archive(record, binding)?;
            } else {
                let terminal = TerminalManagement::open(self.system.path())
                    .map_err(crate::engine_backend::error)?;
                terminal::require_preview(&terminal, &self.identity.principal, record)?;
            }
            return Ok(None);
        }
        let engine = Engine::open_storage_held(
            self.system.try_clone().map_err(storage_error)?, None,
        ).map_err(crate::engine_backend::error)?;
        self.engine_result(engine, record).map(Some)
    }
    fn engine_result(
        &self,
        engine: Engine,
        record: &StoredReadmission,
    ) -> Result<OperationResult, ApiError> {
        self.actor(record)?;
        crate::setup_recovery::SetupRecovery::from_held_control(
            self.control.try_clone().map_err(storage_error)?,
            self.management.path().to_owned(),
        )?
        .require_recovered_engine(&engine)?;
        let backend = EngineBackend::new(engine);
        backend.set_management_auth(self.auth.clone())?;
        backend
            .storage_readmission_result(&self.identity, record)?
            .ok_or_else(|| {
                recovery("active storage has no exact original terminal readmission publication")
            })
    }
    /// Query does not complete a pending resume, supply a target directory, or
    /// treat a cached Completed state as a native publication receipt.
    pub fn status(&self, operation: OperationId) -> Result<ReadmissionStatus, ApiError> {
        self.check()?;
        let directory = open(self.management.path())?.ok_or_else(missing)?;
        let mut record = read_record(&directory, operation)?.ok_or_else(missing)?;
        if let Some(result) = self.query_result(&mut record)? {
            finish(&mut record, result);
        } else if record.status.state == ReadmissionState::Completed {
            return Err(recovery(
                "saved readmission completion has no admitted native receipt",
            ));
        } else {
            record.status.state = ReadmissionState::RecoveryRequired;
        }
        self.check()?;
        Ok(record.status)
    }
    /// Only the exact preview/original operation reaches the explicit target
    /// open, retained seal checks, new epoch and actual native publication.
    pub fn commit(&self, request: ReadmissionCommit) -> Result<ReadmissionStatus, ApiError> {
        self.settled_other()?;
        let directory = self
            .management
            .create_child(
                &EntryName::new(DIRECTORY).expect("invariant: fixed readmission directory"),
            )
            .map_err(storage_error)?;
        let operation = request.preview.request.operation_id;
        let mut record = if let Some(record) = read_record(&directory, operation)? {
            self.actor(&record)?;
            if record.status.preview != request.preview {
                return Err(ApiError::conflict(
                    ErrorCode::IdempotencyKeyReused,
                    "original readmission preview differs",
                ));
            }
            record
        } else {
            let (preview, config) = self.prepare(request.preview.request.clone())?;
            if preview != request.preview {
                return Err(ApiError::conflict(
                    ErrorCode::GenerationMismatch,
                    "terminal readmission source changed; no target was opened",
                ));
            }
            StoredReadmission::prepared(preview, config)
        };
        self.actor(&record)?;
        if let Some(result) = self.query_result(&mut record)? {
            finish(&mut record, result);
            save(&directory, &record)?;
            return Ok(record.status);
        }
        if record.status.state == ReadmissionState::Completed {
            return Err(recovery(
                "readmission completion lost its exact native publication",
            ));
        }
        if read::<OperationId>(&directory, &latest_name())?.is_some_and(|old| old != operation) {
            require_no_pending_readmission(self.management.path())?;
        }
        record.status.state = ReadmissionState::Running;
        record.status.failure = None;
        save(&directory, &record)?;
        write(&directory, &latest_name(), &operation)?;
        let outcome = self.resume(&directory, &mut record);
        match outcome {
            Ok(result) => {
                finish(&mut record, result);
                save(&directory, &record)?;
                self.check()?;
                Ok(record.status)
            }
            Err(error) => {
                record.status.state = ReadmissionState::RecoveryRequired;
                record.status.failure = Some(ReadmissionFailure {
                    code: error.code,
                    message: error.message.clone(),
                });
                save(&directory, &record)?;
                Err(ApiError::new(http::StatusCode::SERVICE_UNAVAILABLE, ErrorCode::RecoveryRequired,
                    format!("terminal readmission {operation} needs its original continuation; no rollback is claimed"))
                    .with_source(Arc::new(error)))
            }
        }
    }
    fn resume(
        &self,
        directory: &PrivateDirectory,
        record: &mut StoredReadmission,
    ) -> Result<OperationResult, ApiError> {
        self.actor(record)?;
        let proof = TerminalManagement::metadata_resume_recovery(self.system.path())
            .map_err(crate::engine_backend::error)?;
        let pending = if let Some(proof) = proof {
            let binding = terminal::recovery_binding(&proof, record)?;
            terminal::bind_archive(record, binding)?;
            save(directory, record)?;
            self.check()?;
            let owner =
                TerminalManagement::open_metadata_resume_checked(self.system.path(), &proof)
                    .map_err(crate::engine_backend::error)?;
            match proof.phase {
                TerminalMetadataResumePhase::CompleteGeneration => {
                    record.status.phase = ReadmissionPhase::Activating;
                    save(directory, record)?;
                    self.check()?;
                    let engine = owner
                        .complete_metadata_resume_held(None)
                        .map_err(crate::engine_backend::error)?;
                    return self.engine_result(engine, record);
                }
                TerminalMetadataResumePhase::ResupplyDirectories => {
                    let target = crate::engine_backend::storage_readmission::open_target(record)?;
                    self.check()?;
                    owner
                        .reopen_metadata_resume_held(vec![target])
                        .map_err(crate::engine_backend::error)?
                }
            }
        } else {
            let owner = TerminalManagement::open(self.system.path())
                .map_err(crate::engine_backend::error)?;
            terminal::require_preview(&owner, &self.identity.principal, record)?;
            record.status.phase = ReadmissionPhase::Fencing;
            save(directory, record)?;
            let target = crate::engine_backend::storage_readmission::open_target(record)?;
            self.check()?;
            owner
                .prepare_disk_readmission(
                    &record.status.preview.target.disk_id,
                    &self.identity.principal,
                    record.status.operation_id,
                    target,
                )
                .map_err(crate::engine_backend::error)?
        };
        self.activate(directory, record, pending)
    }
    fn activate(
        &self,
        directory: &PrivateDirectory,
        record: &mut StoredReadmission,
        mut pending: TerminalMetadataResume,
    ) -> Result<OperationResult, ApiError> {
        self.actor(record)?;
        let binding = terminal::pending_binding(&pending, record)?;
        terminal::bind_archive(record, binding)?;
        record.status.phase = ReadmissionPhase::Archiving;
        save(directory, record)?;
        self.check()?;
        pending
            .synchronize()
            .map_err(crate::engine_backend::error)?;
        record.status.phase = ReadmissionPhase::Activating;
        save(directory, record)?;
        self.check()?;
        let engine = pending.activate().map_err(crate::engine_backend::error)?;
        self.engine_result(engine, record)
    }
}
fn recovery(message: &'static str) -> ApiError {
    ApiError::conflict(ErrorCode::RecoveryRequired, message)
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/management/exclusive_terminal_readmission.rs"]
mod tests;
