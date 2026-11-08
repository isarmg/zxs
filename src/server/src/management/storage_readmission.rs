//! Durable readmission intent. Completion comes only from admitted core history,
//! never the mutable local phase cache or the continued existence of a directory.
use super::storage_addition::{
    ArchiveBinding, configuration_digest, digest, latest_name, name, read, storage_error, write,
};
use super::*;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use storage_contract::{Config, ErrorCode, OperationId, OperationResult, storage_readmission::*};

const DIRECTORY: &str = "storage-readmissions";

#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "storage-directory-readmission")]
    Readmission,
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct StoredReadmission {
    format: Format,
    pub(super) principal: String,
    pub(crate) original: Config,
    pub(crate) archive: Option<ArchiveBinding>,
    pub(crate) status: ReadmissionStatus,
}

impl StoredReadmission {
    pub(super) fn prepared(preview: ReadmissionPreview, original: Config) -> Self {
        Self {
            format: Format::Readmission,
            principal: preview.principal.clone(),
            original,
            archive: None,
            status: ReadmissionStatus {
                operation_id: preview.request.operation_id,
                state: ReadmissionState::Running,
                phase: ReadmissionPhase::Prepared,
                preview,
                failure: None,
                result: None,
            },
        }
    }
    pub(super) fn target_config(&self) -> Config {
        let mut target = self.original.clone();
        if self.status.preview.authority.is_terminal() {
            for disk in &mut target.disks {
                disk.metadata_member = disk.disk_id == self.status.preview.target.disk_id;
            }
            return target;
        }
        target
            .disks
            .iter_mut()
            .find(|disk| disk.disk_id == self.status.preview.target.disk_id)
            .expect("invariant: admitted readmission contains its exact target")
            .metadata_member = self.status.preview.metadata_member;
        target
    }
}

fn open(root: &Path) -> Result<Option<PrivateDirectory>, ApiError> {
    match PrivateDirectory::open_existing(root.join(DIRECTORY)) {
        Ok(directory) => Ok(Some(directory)),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(storage_error(error)),
    }
}
pub(super) fn save(
    directory: &PrivateDirectory,
    record: &StoredReadmission,
) -> Result<(), ApiError> {
    write(directory, &name(record.status.operation_id), record)
}
fn missing() -> ApiError {
    ApiError::new(
        http::StatusCode::NOT_FOUND,
        ErrorCode::OperationNotFound,
        "storage readmission was not found",
    )
}
fn read_record(
    directory: &PrivateDirectory,
    operation: OperationId,
) -> Result<Option<StoredReadmission>, ApiError> {
    let Some(record) = read::<StoredReadmission>(directory, &name(operation))? else {
        return Ok(None);
    };
    let corrupt = || {
        ApiError::conflict(
            ErrorCode::ObjectCorrupt,
            "readmission record does not bind its original actor, request and source",
        )
    };
    let mut preview = record.status.preview.clone();
    let expected = std::mem::take(&mut preview.preview_digest);
    let disk = record
        .original
        .disks
        .iter()
        .find(|disk| disk.disk_id == preview.target.disk_id)
        .ok_or_else(corrupt)?;
    let completed = record.status.state == ReadmissionState::Completed;
    if record.status.operation_id != operation
        || preview.request.operation_id != operation
        || digest(&preview) != expected
        || preview.principal != record.principal
        || configuration_digest(&record.original) != preview.configuration_digest
        || preview.target.pool_id != record.original.pool_id
        || preview.request.expected_disk_id != preview.target.disk_id
        || preview.request.slot_id != preview.target.slot_id
        || disk.slot_id != preview.target.slot_id
        || disk.binding_generation != preview.target.binding_generation
        || disk.root.as_path() != Path::new(&preview.target.root_path)
        || preview.registry_generation == 0
        || (preview.authority.is_terminal() && !preview.metadata_member)
        || completed != (record.status.phase == ReadmissionPhase::Completed)
        || completed != record.status.result.is_some()
        || (completed && record.status.failure.is_some())
    {
        return Err(corrupt());
    }
    storage_engine::ValidatedConfig::try_from(record.original.clone())
        .map_err(crate::engine_backend::error)?;
    if let Some(result) = &record.status.result {
        crate::engine_backend::storage_readmission::exact_result(
            result.clone(),
            &record,
            record.archive.as_ref(),
        )?;
    }
    Ok(Some(record))
}
pub(super) fn finish(record: &mut StoredReadmission, result: OperationResult) {
    record.status.state = ReadmissionState::Completed;
    record.status.phase = ReadmissionPhase::Completed;
    record.status.result = Some(result);
    record.status.failure = None;
}

impl ManagementBackend {
    pub(super) fn storage_readmission(
        &self,
        action: &'static str,
        input: Value,
        identity: &Identity,
    ) -> Result<Value, ApiError> {
        self.inner.current_management_identity(identity)?;
        if action == "disk_registrations" {
            return self.registration_snapshot(identity).map(|snapshot| {
                json_codec::to_value(snapshot).expect("invariant: registry snapshot serializes")
            });
        }
        if action == "disk_readmit_status" {
            #[derive(Deserialize)]
            #[serialization(deny_unknown_fields)]
            struct Query {
                operation_id: Option<OperationId>,
            }
            let query: Query = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            return self
                .readmission_status(identity, query.operation_id)
                .map(|status| {
                    json_codec::to_value(status).expect("invariant: readmission status serializes")
                });
        }
        let _gate = self
            .inner
            .storage_addition
            .try_lock()
            .map_err(|error| match error {
                std::sync::TryLockError::WouldBlock => ApiError::conflict(
                    ErrorCode::StorageRetryRunning,
                    "a storage lifecycle operation is running; query its original operation",
                ),
                std::sync::TryLockError::Poisoned(_) => ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "storage lifecycle ownership failed; restart to recover the original intent",
                ),
            })?;
        self.inner.ensure_running()?;
        if self.inner.recovering.load(Ordering::SeqCst) {
            return Err(ApiError::conflict(
                ErrorCode::StorageRetryRunning,
                "wait for current storage recovery before readmitting a registration",
            ));
        }
        if action == "disk_readmit_preview" {
            self.require_no_pending_addition()?;
            self.require_settled_exit()?;
            self.require_no_pending_readmission()?;
            let request: ReadmissionRequest =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            return self
                .preview_readmission(identity, request)
                .map(|(preview, _)| {
                    json_codec::to_value(preview)
                        .expect("invariant: readmission preview serializes")
                });
        }
        let parent =
            PrivateDirectory::open_existing(&self.inner.management_root).map_err(storage_error)?;
        let directory = parent
            .create_child(
                &EntryName::new(DIRECTORY).expect("invariant: static readmission directory"),
            )
            .map_err(storage_error)?;
        let mut admitted = true;
        let mut record = match action {
            "disk_readmit_commit" => {
                let request: ReadmissionCommit =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let operation = request.preview.request.operation_id;
                if let Some(saved) = read_record(&directory, operation)? {
                    if saved.principal != identity.principal
                        || saved.status.preview != request.preview
                    {
                        return Err(ApiError::conflict(
                            ErrorCode::IdempotencyKeyReused,
                            "operation_id already binds another readmission request or actor",
                        ));
                    }
                    saved
                } else {
                    self.require_no_pending_addition()?;
                    self.require_settled_exit()?;
                    self.require_no_pending_readmission()?;
                    let (preview, original) =
                        self.preview_readmission(identity, request.preview.request.clone())?;
                    if preview != request.preview {
                        return Err(ApiError::conflict(
                            ErrorCode::GenerationMismatch,
                            "readmission preview changed; review the current Disabled registration and authority",
                        ));
                    }
                    admitted = false;
                    StoredReadmission::prepared(preview, original)
                }
            }
            "disk_readmit_resume" => {
                let request: ReadmissionResume =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                read_record(&directory, request.operation_id)?.ok_or_else(missing)?
            }
            _ => return Err(ApiError::invalid("unknown storage readmission action")),
        };
        if record.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        if let Some(result) = self.readmission_result(identity, &record)? {
            finish(&mut record, result);
            save(&directory, &record)?;
            return Ok(json_codec::to_value(record.status)
                .expect("invariant: readmission status serializes"));
        }
        if record.status.state == ReadmissionState::Completed {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "saved readmission completion has no admitted permanent core receipt",
            ));
        }
        self.require_no_pending_addition()?;
        self.require_settled_exit()?;
        let latest = read::<OperationId>(&directory, &latest_name())?;
        if latest.is_some_and(|latest| latest != record.status.operation_id) {
            self.require_no_pending_readmission()?;
        }
        if record.status.preview.authority.is_terminal() {
            return self.execute_terminal_readmission(identity, &directory, record);
        }
        let mut linked = latest == Some(record.status.operation_id);
        let intent = record.clone();
        let backend = self.readmission_backend(identity, &intent)?;
        let outcome = backend.run_storage_readmission(identity, &intent, |phase, archive| {
            if let Some(archive) = archive {
                record.archive = Some(archive);
            }
            record.status.phase = phase;
            record.status.state = ReadmissionState::Running;
            record.status.failure = None;
            save(&directory, &record)?;
            admitted = true;
            if !linked {
                write(&directory, &latest_name(), &record.status.operation_id)?;
                linked = true;
            }
            Ok(())
        });
        match outcome {
            Ok(result) => {
                finish(&mut record, result);
                save(&directory, &record)?;
                crate::lock(&self.inner.disk_observation).take();
                self.inner.current_management_identity(identity)?;
                *crate::lock(&self.inner.capabilities) = backend.capabilities()?;
                let storage = if backend.read_only()? {
                    StorageState::ReadOnly
                } else {
                    StorageState::Ready
                };
                self.inner.set(Some(backend), storage);
                Ok(json_codec::to_value(record.status)
                    .expect("invariant: readmission status serializes"))
            }
            Err(error) => {
                if !admitted {
                    return Err(error);
                }
                record.status.state = ReadmissionState::RecoveryRequired;
                record.status.failure = Some(ReadmissionFailure {
                    code: error.code,
                    message: error.message.clone(),
                });
                if let Err(persistence) = save(&directory, &record) {
                    tracing::error!(error = ?error, persistence = ?persistence, "readmission failed and its diagnostic could not be synchronized");
                    return Err(persistence);
                }
                Err(ApiError::new(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ErrorCode::RecoveryRequired,
                    format!(
                        "readmission {} needs recovery; query and continue the same operation",
                        record.status.operation_id
                    ),
                )
                .with_source(Arc::new(error)))
            }
        }
    }

    fn preview_readmission(
        &self,
        identity: &Identity,
        request: ReadmissionRequest,
    ) -> Result<(ReadmissionPreview, Config), ApiError> {
        if let Some(terminal) = self.inner.terminal(identity)? {
            return super::terminal_readmission::preview(&terminal, &identity.principal, request);
        }
        self.inner
            .addition_backend()?
            .preview_storage_readmission(identity, request)
    }

    /// Only an explicit continuation may finish an authenticated recovery plan.
    /// Status/preview calls never enter this path or replace a live Engine owner.
    fn readmission_backend(
        &self,
        identity: &Identity,
        intent: &StoredReadmission,
    ) -> Result<Arc<EngineBackend>, ApiError> {
        if let Some(backend) = crate::lock(&self.inner.state)
            .backend
            .active_engine()
            .cloned()
        {
            return Ok(backend);
        }
        let _worker = crate::lock(&self.inner.recovery_worker);
        self.inner.current_management_identity(identity)?;
        if self.inner.recovering.load(Ordering::SeqCst) {
            return Err(ApiError::conflict(
                ErrorCode::StorageRetryRunning,
                "storage recovery already owns the engine",
            ));
        }
        {
            let mut state = crate::lock(&self.inner.state);
            if !matches!(state.backend, BackendState::Unopened) {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "readmission cannot replace terminal or draining authority; continue its original operation",
                ));
            }
            state.backend = BackendState::ChangingAuthority;
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
        }
        let recovered = (|| {
            let engine = if let Some(source) =
                storage_engine::Engine::readmission_recovery_status(&self.inner.data_root)
                    .map_err(crate::engine_backend::error)?
            {
                crate::engine_backend::storage_readmission::verify_status(&source, intent)?;
                storage_engine::Engine::resume_disk_readmission_transition(
                    &self.inner.data_root,
                    &source,
                    self.inner.retained_workspace_limits,
                )
            } else {
                storage_engine::Engine::open_for_recovery_with_workspace_limits(
                    &self.inner.data_root,
                    self.inner.retained_workspace_limits,
                )
            }
            .map_err(crate::engine_backend::error)?;
            let backend = Arc::new(EngineBackend::new(engine));
            self.inner.current_management_identity(identity)?;
            let auth = crate::lock(&self.inner.auth)
                .clone()
                .ok_or_else(ApiError::unauthorized)?;
            backend.set_management_auth(crate::lock(&auth).reader())?;
            Ok::<_, ApiError>(backend)
        })();
        let mut state = crate::lock(&self.inner.state);
        state.backend = match &recovered {
            Ok(backend) => BackendState::Engine(Arc::clone(backend)),
            Err(_) => BackendState::Unopened,
        };
        state.admitted = false;
        state.status = StorageState::RecoveryRequired;
        state.observed_at = std::time::Instant::now();
        recovered
    }

    fn registration_snapshot(&self, identity: &Identity) -> Result<RegistrationSnapshot, ApiError> {
        let _gate = match self.inner.storage_addition.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Ok(RegistrationSnapshot::Unadmitted);
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "registry admission requires restart recovery",
                ));
            }
        };
        if let Some(terminal) = self.inner.terminal(identity)? {
            return terminal_registration_snapshot(&terminal);
        }
        let backend = crate::lock(&self.inner.state)
            .backend
            .active_engine()
            .cloned();
        match backend {
            Some(backend) => backend.storage_registration_snapshot(identity),
            None => Ok(RegistrationSnapshot::Unadmitted),
        }
    }

    pub(super) fn require_no_pending_readmission(&self) -> Result<(), ApiError> {
        require_no_pending_readmission(&self.inner.management_root)
    }

    pub(super) fn readmission_result(
        &self,
        identity: &Identity,
        intent: &StoredReadmission,
    ) -> Result<Option<OperationResult>, ApiError> {
        // A later final-voter exit retains earlier readmission receipts. Reading
        // authenticated terminal history does not reopen that retired target.
        if let Some(terminal) = self.inner.terminal(identity)? {
            if terminal.original_configuration().pool_id != intent.status.preview.target.pool_id {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "readmission receipt belongs to another pool",
                ));
            }
            let operation = intent.status.operation_id;
            let Some(result) = terminal.operation(operation) else {
                return Ok(None);
            };
            if terminal
                .state()
                .operation_principals
                .get(&operation)
                .map(String::as_str)
                != Some(identity.principal.as_str())
            {
                return Err(ApiError::forbidden());
            }
            return crate::engine_backend::storage_readmission::exact_result(
                result.clone(),
                intent,
                intent.archive.as_ref(),
            )
            .map(Some);
        }
        let backend = crate::lock(&self.inner.state).backend.engine().cloned();
        match backend {
            Some(backend) => backend.storage_readmission_result(identity, intent),
            None => Ok(None),
        }
    }

    fn readmission_status(
        &self,
        identity: &Identity,
        operation: Option<OperationId>,
    ) -> Result<ReadmissionStatusResponse, ApiError> {
        let Some(directory) = open(&self.inner.management_root)? else {
            return if operation.is_some() {
                Err(missing())
            } else {
                Ok(ReadmissionStatusResponse { operation: None })
            };
        };
        let operation = match operation {
            Some(operation) => Some(operation),
            None => read(&directory, &latest_name())?,
        };
        let Some(operation) = operation else {
            return Ok(ReadmissionStatusResponse { operation: None });
        };
        let mut record = read_record(&directory, operation)?.ok_or_else(missing)?;
        if record.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        let gate = match self.inner.storage_addition.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "readmission ownership requires restart recovery",
                ));
            }
        };
        if gate.is_some() {
            if let Some(result) = self.readmission_result(identity, &record)? {
                finish(&mut record, result);
            } else if record.status.state == ReadmissionState::Completed {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "saved readmission completion cannot be verified from admitted core authority",
                ));
            } else {
                record.status.state = ReadmissionState::RecoveryRequired;
            }
        } else if record.status.state == ReadmissionState::Completed {
            return Err(ApiError::conflict(
                ErrorCode::StorageRetryRunning,
                "wait for the active lifecycle operation to verify this completion",
            ));
        }
        Ok(ReadmissionStatusResponse {
            operation: Some(record.status),
        })
    }
}

pub(super) fn configuration_edges(
    root: &Path,
    chain: &mut super::storage_configuration::ConfigurationChain,
) -> Result<(), ApiError> {
    super::storage_configuration::records(&root.join(DIRECTORY), read_record, |record, latest| {
        let original = configuration_digest(&record.original);
        let target = configuration_digest(&record.target_config());
        if latest {
            chain.tip(original.clone());
            chain.tip(target.clone());
        }
        chain.edge(original, target);
        Ok(())
    })
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/storage_readmission.rs"]
mod tests;

pub(super) fn require_no_pending_readmission(root: &Path) -> Result<(), ApiError> {
    let Some(directory) = open(root)? else {
        return Ok(());
    };
    if let Some(operation) = read::<OperationId>(&directory, &latest_name())? {
        let record = read_record(&directory, operation)?.ok_or_else(|| {
            ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "latest readmission intent is missing",
            )
        })?;
        if record.status.state != ReadmissionState::Completed {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "continue the original pending readmission before starting another plan",
            ));
        }
    }
    Ok(())
}

pub(super) fn terminal_registration_snapshot(
    terminal: &storage_engine::TerminalManagement,
) -> Result<RegistrationSnapshot, ApiError> {
    Ok(RegistrationSnapshot::TerminalManagement {
        registrations: terminal
            .registered_disks()
            .map(|disk| {
                let mut entry =
                    crate::engine_backend::storage_readmission::registration_entry(disk);
                entry.readmission = terminal
                    .readmission_metadata_member(&entry.disk_id)
                    .map_err(crate::engine_backend::error)?
                    .map(|_| ReadmissionRole::MetadataMember);
                Ok(entry)
            })
            .collect::<Result<Vec<_>, ApiError>>()?,
    })
}
