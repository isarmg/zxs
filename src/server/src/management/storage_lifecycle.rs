//! Durable exit intent is coordination state, never D3 publication authority.
use super::storage_addition::{
    ArchiveBinding, configuration_digest, digest, latest_name, name, read, storage_error, write,
};
use super::*;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use storage_contract::{
    Config, ErrorCode, OperationId, OperationResult, storage_addition::AdditionFailure,
    storage_lifecycle::*,
};

const DIRECTORY: &str = "storage-exits";
#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "storage-directory-exit")]
    Exit,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct StoredExit {
    format: Format,
    principal: String,
    original: Config,
    pub(crate) archive: Option<ArchiveBinding>,
    pub(crate) status: ExitStatus,
}
impl StoredExit {
    fn target_config(&self) -> Config {
        let mut target = self.original.clone();
        target
            .disks
            .iter_mut()
            .find(|disk| disk.disk_id == self.status.preview.target.disk_id)
            .expect("invariant: admitted exit contains its exact target")
            .metadata_member = false;
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
fn save(directory: &PrivateDirectory, record: &StoredExit) -> Result<(), ApiError> {
    write(directory, &name(record.status.operation_id), record)
}
fn read_record(
    directory: &PrivateDirectory,
    operation: OperationId,
) -> Result<Option<StoredExit>, ApiError> {
    let Some(record) = read::<StoredExit>(directory, &name(operation))? else {
        return Ok(None);
    };
    let corrupt = || {
        ApiError::conflict(
            ErrorCode::ObjectCorrupt,
            "exit control record does not bind its original request and source",
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
    if record.status.operation_id != operation
        || preview.request.operation_id != operation
        || digest(&preview) != expected
        || configuration_digest(&record.original) != preview.configuration_digest
        || preview.target.pool_id != record.original.pool_id
        || preview.request.expected_disk_id != preview.target.disk_id
        || preview.request.slot_id != preview.target.slot_id
        || disk.slot_id != preview.target.slot_id
        || disk.binding_generation != preview.target.binding_generation
        || disk.root.as_path() != Path::new(&preview.target.root_path)
        || preview.original_metadata_members
            != record
                .original
                .disks
                .iter()
                .filter(|disk| disk.metadata_member)
                .count()
        || preview.target_metadata_members
            != preview.original_metadata_members - usize::from(disk.metadata_member)
        || (preview.authority == ExitAuthority::TerminalManagement)
            != (preview.target_metadata_members == 0)
        || (record.status.state == ExitState::Completed)
            != (record.status.phase == ExitPhase::Completed)
        || (record.status.state == ExitState::Completed) != record.status.result.is_some()
    {
        return Err(corrupt());
    }
    storage_engine::ValidatedConfig::try_from(record.original.clone())
        .map_err(crate::engine_backend::error)?;
    if let Some(result) = &record.status.result
        && (result.operation_id != operation
            || result.kind != preview.request.action.result_kind()
            || result.commit_state != storage_contract::CommitState::Published
            || record
                .archive
                .as_ref()
                .is_none_or(|archive| archive.transition_hash != result.request_digest)
            || record.status.failure.is_some())
    {
        return Err(corrupt());
    }
    Ok(Some(record))
}
fn missing() -> ApiError {
    ApiError::new(
        http::StatusCode::NOT_FOUND,
        ErrorCode::OperationNotFound,
        "storage lifecycle operation was not found",
    )
}
fn finish(record: &mut StoredExit, result: OperationResult) {
    record.status.phase = ExitPhase::Completed;
    record.status.state = ExitState::Completed;
    record.status.result = Some(result);
    record.status.failure = None;
}

impl ManagementBackend {
    pub(super) fn require_settled_exit(&self) -> Result<(), ApiError> {
        require_settled_exit(&self.inner.management_root)
    }

    pub(super) fn storage_exit(
        &self,
        action: &'static str,
        input: Value,
        identity: &Identity,
    ) -> Result<Value, ApiError> {
        self.inner.current_management_identity(identity)?;
        if action == "disk_exit_status" {
            #[derive(Deserialize)]
            #[serialization(deny_unknown_fields)]
            struct Query {
                operation_id: Option<OperationId>,
            }
            let query: Query = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            return self
                .exit_status(identity, query.operation_id)
                .map(|status| {
                    json_codec::to_value(status).expect("invariant: exit status serializes")
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
                "wait for the current storage recovery before changing registrations",
            ));
        }
        self.inner.current_management_identity(identity)?;
        if action == "disk_exit_preview" {
            let backend = self.inner.addition_backend()?;
            self.require_no_pending_addition()?;
            self.require_no_pending_readmission()?;
            self.require_settled_exit()?;
            let request: ExitRequest =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            return backend
                .preview_storage_exit(identity, request)
                .map(|(preview, _)| {
                    json_codec::to_value(preview).expect("invariant: exit preview serializes")
                });
        }
        let parent =
            PrivateDirectory::open_existing(&self.inner.management_root).map_err(storage_error)?;
        let directory = parent
            .create_child(&EntryName::new(DIRECTORY).expect("invariant: static exit directory"))
            .map_err(storage_error)?;
        let mut admitted = true;
        let mut record = match action {
            "disk_exit_commit" => {
                let request: ExitCommit =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let operation = request.preview.request.operation_id;
                if let Some(saved) = read_record(&directory, operation)? {
                    if saved.principal != identity.principal
                        || saved.status.preview != request.preview
                    {
                        return Err(ApiError::conflict(
                            ErrorCode::IdempotencyKeyReused,
                            "operation_id already binds another exit request or actor",
                        ));
                    }
                    saved
                } else {
                    self.require_no_pending_addition()?;
                    self.require_no_pending_readmission()?;
                    self.require_settled_exit()?;
                    let (preview, original) = self
                        .inner
                        .addition_backend()?
                        .preview_storage_exit(identity, request.preview.request.clone())?;
                    if preview != request.preview {
                        return Err(ApiError::conflict(
                            ErrorCode::GenerationMismatch,
                            "exit preview changed; review the current registration and source",
                        ));
                    }
                    let record = StoredExit {
                        format: Format::Exit,
                        principal: identity.principal.clone(),
                        original,
                        archive: None,
                        status: ExitStatus {
                            operation_id: operation,
                            state: ExitState::Running,
                            phase: ExitPhase::Prepared,
                            preview,
                            failure: None,
                            result: None,
                        },
                    };
                    // Persist only inside the engine-locked callback after
                    // rechecking the source. A stale preview must not leave an
                    // unfenced intent blocking all later plans.
                    admitted = false;
                    record
                }
            }
            "disk_exit_resume" => {
                let request: ExitResume =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                read_record(&directory, request.operation_id)?.ok_or_else(missing)?
            }
            _ => return Err(ApiError::invalid("unknown storage lifecycle action")),
        };
        if record.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        if let Some(result) = self.storage_exit_result(identity, &mut record)? {
            finish(&mut record, result);
            save(&directory, &record)?;
            return Ok(
                json_codec::to_value(record.status).expect("invariant: exit status serializes")
            );
        }
        if record.status.state == ExitState::Completed {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "saved exit completion has no admitted permanent engine receipt",
            ));
        }
        self.require_no_pending_addition()?;
        self.require_no_pending_readmission()?;
        let latest = read::<OperationId>(&directory, &latest_name())?;
        if latest.is_some_and(|latest| latest != record.status.operation_id) {
            self.require_settled_exit()?;
        }
        let mut linked = latest == Some(record.status.operation_id);
        let intent = record.clone();
        let progress = |phase, archive| {
            if let Some(archive) = archive {
                record.archive = Some(archive);
            }
            record.status.phase = phase;
            record.status.state = ExitState::Running;
            record.status.failure = None;
            save(&directory, &record)?;
            admitted = true;
            if !linked {
                write(&directory, &latest_name(), &record.status.operation_id)?;
                linked = true;
            }
            Ok(())
        };
        let outcome = match intent.status.preview.authority {
            ExitAuthority::ActiveMembership => self
                .inner
                .addition_backend()?
                .run_storage_exit(identity, &intent, progress),
            ExitAuthority::TerminalManagement => {
                self.inner.run_terminal_exit(identity, &intent, progress)
            }
        };
        match outcome {
            Ok(result) => {
                let result = crate::engine_backend::storage_lifecycle::exact_result(
                    result,
                    &record,
                    record.archive.as_ref(),
                )?;
                finish(&mut record, result);
                save(&directory, &record)?;
                crate::lock(&self.inner.disk_observation).take();
                Ok(json_codec::to_value(record.status).expect("invariant: exit status serializes"))
            }
            Err(error) => {
                if !admitted {
                    return Err(error);
                }
                record.status.state = ExitState::RecoveryRequired;
                record.status.failure = Some(AdditionFailure {
                    code: error.code,
                    message: error.message.clone(),
                });
                if let Err(persistence) = save(&directory, &record) {
                    tracing::error!(error = ?error, persistence = ?persistence, "exit failed and its diagnostic could not be synchronized");
                    return Err(persistence);
                }
                Err(ApiError::new(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ErrorCode::RecoveryRequired,
                    format!(
                        "exit {} needs recovery; query and continue the same operation",
                        record.status.operation_id
                    ),
                )
                .with_source(Arc::new(error)))
            }
        }
    }

    fn storage_exit_result(
        &self,
        identity: &Identity,
        record: &mut StoredExit,
    ) -> Result<Option<OperationResult>, ApiError> {
        if self.inner.terminal(identity)?.is_some() {
            return self.inner.terminal_exit_result(identity, record);
        }
        let backend = crate::lock(&self.inner.state).backend.engine().cloned();
        match backend {
            Some(backend) => backend.storage_exit_result(identity, record),
            None => Ok(None),
        }
    }

    fn exit_status(
        &self,
        identity: &Identity,
        operation: Option<OperationId>,
    ) -> Result<ExitStatusResponse, ApiError> {
        let Some(directory) = open(&self.inner.management_root)? else {
            return if operation.is_some() {
                Err(missing())
            } else {
                Ok(ExitStatusResponse { operation: None })
            };
        };
        let operation = match operation {
            Some(operation) => Some(operation),
            None => read(&directory, &latest_name())?,
        };
        let Some(operation) = operation else {
            return Ok(ExitStatusResponse { operation: None });
        };
        let mut record = read_record(&directory, operation)?.ok_or_else(missing)?;
        if record.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        let running = match self.inner.storage_addition.try_lock() {
            Ok(_guard) => false,
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        };
        if running {
            if record.status.state == ExitState::Completed {
                return Err(ApiError::conflict(
                    ErrorCode::StorageRetryRunning,
                    "wait for the active lifecycle operation to verify this completion",
                ));
            }
        } else {
            if let Some(result) = self.storage_exit_result(identity, &mut record)? {
                finish(&mut record, result);
            } else if record.status.state == ExitState::Completed {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "saved exit completion cannot be verified from admitted engine authority",
                ));
            } else {
                record.status.state = ExitState::RecoveryRequired;
            }
        }
        Ok(ExitStatusResponse {
            operation: Some(record.status),
        })
    }
}

pub(super) fn configuration_edges(
    root: &Path,
    chain: &mut super::storage_configuration::ConfigurationChain,
) -> Result<(), ApiError> {
    super::storage_configuration::records(&root.join(DIRECTORY), read_record, |record, latest| {
        if record.status.preview.authority == ExitAuthority::TerminalManagement {
            return Ok(());
        }
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
#[path = "../../../../test/server/unit/management/storage_lifecycle.rs"]
mod tests;

pub(super) fn require_settled_exit(root: &Path) -> Result<(), ApiError> {
    let Some(directory) = open(root)? else {
        return Ok(());
    };
    let Some(operation) = read::<OperationId>(&directory, &latest_name())? else {
        return Ok(());
    };
    let record = read_record(&directory, operation)?.ok_or_else(missing)?;
    if record.status.state != ExitState::Completed {
        return Err(ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "continue the original pending exit before starting another storage plan",
        ));
    }
    Ok(())
}
