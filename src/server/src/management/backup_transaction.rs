//! Management owns the directory scope; the Engine owns all transaction stages.
use super::*;
use storage_contract::{OperationId, backup_transaction as wire};

impl ManagementBackend {
    fn backup_transaction_backend(
        &self,
        caller: &Identity,
    ) -> Result<Arc<EngineBackend>, ApiError> {
        let current = self.inner.current_management_identity(caller)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        // This is not general business admission. The original backup must stay
        // observable/continuable while its own native source fence blocks writes.
        crate::lock(&self.inner.state).backend.active_engine().cloned().ok_or_else(|| {
            ApiError::new(http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::StorageUnavailable,
                "the original backup engine is unavailable; recover the same pool before continuing")
        })
    }

    pub(super) fn read_backup_context(
        &self,
        identity: &Identity,
    ) -> Result<wire::Context, ApiError> {
        self.inner.current_management_identity(identity)?;
        let backend = self.inner.backend()?;
        let (sequence, source_slots) = backend.backup_transaction_context(identity)?;
        let discovery = self.discovered_disks(identity)?;
        Ok(wire::Context {
            sequence,
            source_slots,
            discovery,
        })
    }

    pub(super) fn read_current_backup(
        &self,
        identity: &Identity,
    ) -> Result<wire::Current, ApiError> {
        let backend = self.backup_transaction_backend(identity)?;
        let current = backend.current_backup_transaction(identity)?;
        if let Some(snapshot) = &current.transaction {
            self.require_backup_serving(&backend, snapshot)?;
        }
        Ok(current)
    }

    pub(super) fn read_backup(
        &self,
        identity: &Identity,
        operation: OperationId,
    ) -> Result<wire::Snapshot, ApiError> {
        let backend = self.backup_transaction_backend(identity)?;
        let snapshot = backend.backup_transaction_status(identity, operation)?;
        self.require_backup_serving(&backend, &snapshot)?;
        Ok(snapshot)
    }

    pub(super) fn begin_backup(
        &self,
        identity: &Identity,
        request: wire::StartRequest,
    ) -> Result<wire::Snapshot, ApiError> {
        // Shares lifecycle admission with existing directory/registration changes.
        let _lifecycle = crate::lock(&self.inner.storage_addition);
        self.inner.current_management_identity(identity)?;
        let backend = self.inner.backend()?;
        let snapshot = backend.start_backup_transaction(
            identity,
            request,
            &self.inner.discovery_root,
            &self.inner.management_root,
        )?;
        self.require_backup_serving(&backend, &snapshot)?;
        Ok(snapshot)
    }

    pub(super) fn advance_backup(
        &self,
        identity: &Identity,
        operation: OperationId,
        command: wire::Command,
    ) -> Result<wire::CommandResponse, ApiError> {
        let _lifecycle = crate::lock(&self.inner.storage_addition);
        if matches!(command, wire::Command::ResumeOriginalTask { .. })
            && matches!(
                crate::lock(&self.inner.state).backend,
                BackendState::Unopened
            )
        {
            self.inner
                .start_backup_resume(identity.clone(), operation)?;
            return Ok(wire::CommandResponse::RecoveryQueued {
                operation_id: operation,
            });
        }
        let backend = self.backup_transaction_backend(identity)?;
        let response = backend.continue_backup_transaction(identity, operation, command)?;
        if let wire::CommandResponse::CancellationRejected { snapshot } = &response {
            self.require_backup_serving(&backend, snapshot)?;
        }
        if matches!(&response, wire::CommandResponse::Observed { snapshot } if snapshot.completion.is_some()) {
            // Core activation is necessary but not sufficient for the service.
            // Keep existing auth replication/capabilities/ready admission owned
            // by its normal recovery worker; GET never schedules it.
            self.inner
                .set(Some(backend), StorageState::RecoveryRequired);
            match self.inner.recover() {
                Ok(()) => {}
                Err(error) if error.code == storage_contract::ErrorCode::StorageRetryRunning => {}
                Err(error) => return Err(error),
            }
            return Ok(wire::CommandResponse::RecoveryQueued {
                operation_id: operation,
            });
        }
        Ok(response)
    }

    fn require_backup_serving(
        &self,
        original: &Arc<EngineBackend>,
        snapshot: &wire::Snapshot,
    ) -> Result<(), ApiError> {
        if snapshot.completion.is_none() {
            return Ok(());
        }
        let admitted = self.inner.backend()?;
        if !Arc::ptr_eq(original, &admitted) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "the backup observation belongs to an engine being replaced; query the same operation",
            ));
        }
        admitted.ready().map(|_| ())
    }
}

impl Inner {
    fn start_backup_resume(
        self: &Arc<Self>,
        identity: Identity,
        operation: OperationId,
    ) -> Result<(), ApiError> {
        let actor = self.current_management_identity(&identity)?;
        if actor.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        let mut worker = crate::lock(&self.recovery_worker);
        self.ensure_running()?;
        if self.recovering.swap(true, Ordering::SeqCst) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::StorageRetryRunning,
                "the owned storage recovery worker is already running",
            ));
        }
        if !matches!(crate::lock(&self.state).backend, BackendState::Unopened) {
            self.recovering.store(false, Ordering::SeqCst);
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::StorageDraining,
                "storage ownership changed; query the original backup operation",
            ));
        }
        let previous = worker.take();
        drop(worker);
        if let Some(previous) = previous {
            previous
                .join()
                .expect("invariant: storage recovery worker does not panic");
        }
        let mut worker = crate::lock(&self.recovery_worker);
        if let Err(error) = self.ensure_running() {
            self.recovering.store(false, Ordering::SeqCst);
            return Err(error);
        }
        let inner = Arc::clone(self);
        match std::thread::Builder::new().name("zxs-backup-recovery".into()).spawn(move || {
            inner.run_recovery(move |inner| {
                let actor = inner.current_management_identity(&identity)?;
                if crate::lock(&inner.state).backend.engine().is_none() {
                    let engine = storage_engine::Engine::resume_backup_transaction_at_with_workspace_limits(
                        &inner.data_root, operation, &actor.principal, inner.retained_workspace_limits,
                    ).map_err(crate::engine_backend::error)?;
                    // Keep all recovered writer owners, but do not admit ordinary
                    // requests until existing primary-auth/capability/ready gates pass.
                    inner.set(Some(Arc::new(EngineBackend::new(engine))), StorageState::RecoveryRequired);
                }
                inner.recover_once()
            });
        }) {
            Ok(handle) => { *worker = Some(handle); Ok(()) }
            Err(source) => { self.recovering.store(false, Ordering::SeqCst); Err(ApiError::io(source)) }
        }
    }
}
