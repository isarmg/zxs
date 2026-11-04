//! Durable control-plane intent; engine receipts remain publication authority.
use super::*;
use data_serialization::{Deserialize, Serialize, de::DeserializeOwned};
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory};
use storage_contract::{Config, DiskConfig, ErrorCode, OperationId, storage_addition::*};

const DIRECTORY: &str = "storage-additions";
const MAX_RECORD: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "storage-directory-addition")]
    Addition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ArchiveBinding {
    pub(crate) archive_path: PathBuf,
    pub(crate) root_hash: String,
    pub(crate) transition_hash: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct StoredAddition {
    format: Format,
    principal: String,
    pub(crate) target: Config,
    pub(crate) archive: Option<ArchiveBinding>,
    pub(crate) status: AdditionStatus,
}

pub(crate) fn configuration_digest(config: &Config) -> String {
    digest(config)
}
fn digest(value: &impl Serialize) -> String {
    let bytes = json_codec::to_vec(value).expect("invariant: typed addition serializes");
    blake3::hash(&bytes).to_hex().to_string()
}
fn storage_error(source: platform_io::fs_safety::Error) -> ApiError {
    ApiError::new(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::IoError,
        "storage addition control record could not be read or synchronized",
    )
    .with_source(source)
}
fn name(operation: OperationId) -> EntryName {
    EntryName::new(format!("{operation}.json")).expect("invariant: typed operation is a filename")
}
fn read<T: DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &EntryName,
) -> Result<Option<T>, ApiError> {
    let bytes = match directory.read_bounded(name, MAX_RECORD) {
        Ok(bytes) => bytes,
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => return Err(storage_error(error)),
    };
    let value = storage_contract::strict_json(&bytes).map_err(ApiError::invalid_source)?;
    json_codec::from_value(value)
        .map(Some)
        .map_err(ApiError::invalid_source)
}
fn read_record(
    directory: &PrivateDirectory,
    operation: OperationId,
) -> Result<Option<StoredAddition>, ApiError> {
    let Some(record) = read::<StoredAddition>(directory, &name(operation))? else {
        return Ok(None);
    };
    let corrupt = || {
        ApiError::conflict(
            ErrorCode::ObjectCorrupt,
            "storage addition record does not bind its original request and target",
        )
    };
    let mut preview = record.status.preview.clone();
    let saved_digest = std::mem::take(&mut preview.preview_digest);
    if record.status.operation_id != operation
        || preview.request.operation_id != operation
        || preview.request.additions.is_empty()
        || preview.request.additions.len() > 32
        || digest(&preview) != saved_digest
        || preview.pool_id != record.target.pool_id
        || preview.added_directories != preview.request.additions.len()
        || (record.status.state == AdditionState::Completed)
            != (record.status.phase == AdditionPhase::Completed)
    {
        return Err(corrupt());
    }
    storage_engine::ValidatedConfig::try_from(record.target.clone())
        .map_err(crate::engine_backend::error)?;
    let mut original = record.target.clone();
    for addition in &preview.request.additions {
        let id_hash = digest(&(operation, &addition.slot_id));
        let id = format!("disk_{}", &id_hash[..32]);
        let index = original
            .disks
            .iter()
            .position(|disk| disk.disk_id == id)
            .ok_or_else(corrupt)?;
        let disk = original.disks.remove(index);
        if disk.slot_id != addition.slot_id
            || disk.root != PathBuf::from(&addition.selection.root_path)
            || disk.level != addition.level
            || disk.upstream_slot_id != addition.upstream_slot_id
            || disk.metadata_member != addition.metadata_member
            || disk.binding_generation != 1
            || disk.capacity_bytes.is_some()
        {
            return Err(corrupt());
        }
    }
    if configuration_digest(&original) != preview.configuration_digest
        || original
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .count()
            != preview.original_metadata_members
        || record
            .target
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .count()
            != preview.target_metadata_members
    {
        return Err(corrupt());
    }
    Ok(Some(record))
}

fn write(
    directory: &PrivateDirectory,
    name: &EntryName,
    value: &impl Serialize,
) -> Result<(), ApiError> {
    let bytes = json_codec::to_vec(value).expect("invariant: addition record serializes");
    if bytes.len() > MAX_RECORD {
        return Err(ApiError::limit(
            "storage addition control record exceeds its bound",
        ));
    }
    AtomicFile::replace(directory, &name.as_relative(), &bytes).map_err(storage_error)
}
fn latest_name() -> EntryName {
    EntryName::new("latest.json").expect("invariant: static addition pointer")
}
fn save(directory: &PrivateDirectory, record: &StoredAddition) -> Result<(), ApiError> {
    write(directory, &name(record.status.operation_id), record)
}
fn conflict(message: &'static str) -> ApiError {
    ApiError::conflict(ErrorCode::IdempotencyKeyReused, message)
}

impl Inner {
    fn addition_backend(&self) -> Result<Arc<EngineBackend>, ApiError> {
        self.ensure_running()?;
        // A fenced pool is deliberately not ready for ordinary business writes.
        // Retain its exact engine for authorized recovery of this operation.
        crate::lock(&self.state).backend.clone().ok_or_else(|| {
            ApiError::new(
                http::StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::StorageUnavailable,
                "open the existing pool before continuing its storage addition",
            )
        })
    }
}

impl ManagementBackend {
    pub(super) fn storage_addition(
        &self,
        action: &'static str,
        input: Value,
        identity: &Identity,
        discovery_root: impl FnOnce() -> Result<String, ApiError>,
    ) -> Result<Value, ApiError> {
        if action == "disk_add_status" {
            #[derive(Deserialize)]
            #[serialization(deny_unknown_fields)]
            struct Query {
                operation_id: Option<OperationId>,
            }
            let query: Query = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            return self
                .addition_status(identity, query.operation_id)
                .map(|status| {
                    json_codec::to_value(status).expect("invariant: addition status serializes")
                });
        }
        let _gate = self
            .inner
            .storage_addition
            .try_lock()
            .map_err(|error| match error {
                std::sync::TryLockError::WouldBlock => ApiError::conflict(
                    ErrorCode::StorageRetryRunning,
                    "a storage addition is running; query its original operation",
                ),
                std::sync::TryLockError::Poisoned(_) => ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "storage addition coordination failed; restart to recover its durable intent",
                ),
            })?;
        self.inner.ensure_running()?;
        if self.inner.recovering.load(Ordering::SeqCst) {
            return Err(ApiError::conflict(
                ErrorCode::StorageRetryRunning,
                "wait for the current storage lifecycle operation before adding directories",
            ));
        }
        let discovery_root = if matches!(action, "disk_add_preview" | "disk_add_commit") {
            Some(discovery_root()?)
        } else {
            None
        };
        let backend = self.inner.addition_backend()?;
        if action == "disk_add_preview" {
            let request: AdditionRequest =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            let (preview, _, _) = self.prepare_addition(
                &backend,
                identity,
                request,
                discovery_root
                    .as_deref()
                    .expect("invariant: preview loads discovery settings"),
            )?;
            return Ok(
                json_codec::to_value(preview).expect("invariant: addition preview serializes")
            );
        }
        let parent =
            PrivateDirectory::open_existing(&self.inner.management_root).map_err(storage_error)?;
        let directory = parent
            .create_child(&EntryName::new(DIRECTORY).expect("invariant: static directory"))
            .map_err(storage_error)?;
        let mut held_directories = None;
        let mut record = if action == "disk_add_commit" {
            let request: AdditionCommit =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            let operation = request.preview.request.operation_id;
            if let Some(saved) = read_record(&directory, operation)? {
                if saved.principal != identity.principal || saved.status.preview != request.preview
                {
                    return Err(conflict(
                        "operation_id already belongs to a different addition",
                    ));
                }
                saved
            } else {
                self.require_settled_addition(&directory)?;
                let (preview, target, directories) = self.prepare_addition(
                    &backend,
                    identity,
                    request.preview.request.clone(),
                    discovery_root
                        .as_deref()
                        .expect("invariant: new commit loads discovery settings"),
                )?;
                if preview != request.preview {
                    return Err(ApiError::conflict(
                        ErrorCode::GenerationMismatch,
                        "addition preview changed; inspect and review the current directories and pool",
                    ));
                }
                held_directories = Some(directories);
                let record = StoredAddition {
                    format: Format::Addition,
                    principal: identity.principal.clone(),
                    target,
                    archive: None,
                    status: AdditionStatus {
                        operation_id: operation,
                        state: AdditionState::Adding,
                        phase: AdditionPhase::Prepared,
                        preview,
                        failure: None,
                    },
                };
                save(&directory, &record)?;
                record
            }
        } else if action == "disk_add_resume" {
            let request: AdditionResume =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            read_record(&directory, request.operation_id)?.ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    ErrorCode::OperationNotFound,
                    "storage addition was not found",
                )
            })?
        } else {
            return Err(ApiError::invalid("unknown storage addition action"));
        };
        if record.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        if record.status.state == AdditionState::Completed {
            return Ok(
                json_codec::to_value(record.status).expect("invariant: addition status serializes")
            );
        }
        if let Some(latest) = read::<OperationId>(&directory, &latest_name())?
            && latest != record.status.operation_id
        {
            self.require_settled_addition(&directory)?;
        }
        let current_digest = configuration_digest(&backend.addition_configuration(identity)?);
        if current_digest != record.status.preview.configuration_digest
            && current_digest != configuration_digest(&record.target)
        {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "this pending operation belongs to a different pool configuration",
            ));
        }
        write(&directory, &latest_name(), &record.status.operation_id)?;
        let intent = record.clone();
        let outcome =
            backend.run_storage_addition(identity, &intent, held_directories, |phase, archive| {
                if let Some(archive) = archive {
                    record.archive = Some(archive);
                }
                record.status.phase = phase;
                record.status.state = AdditionState::Adding;
                record.status.failure = None;
                save(&directory, &record)
            });
        match outcome {
            Ok(_) => {
                record.status.phase = AdditionPhase::Completed;
                record.status.state = AdditionState::Completed;
                record.status.failure = None;
                save(&directory, &record)?;
                self.inner
                    .disk_observation
                    .lock()
                    .expect("invariant: observation lock")
                    .take();
                Ok(json_codec::to_value(record.status)
                    .expect("invariant: addition status serializes"))
            }
            Err(error) => {
                record.status.state = AdditionState::RecoveryRequired;
                record.status.failure = Some(AdditionFailure {
                    code: error.code,
                    message: error.message.clone(),
                });
                if let Err(persistence) = save(&directory, &record) {
                    tracing::error!(error = ?error, persistence = ?persistence, "addition failed and its diagnostic could not be synchronized");
                    return Err(persistence);
                }
                Err(ApiError::new(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ErrorCode::RecoveryRequired,
                    format!(
                        "addition {} needs recovery; query and continue the same operation",
                        record.status.operation_id
                    ),
                )
                .with_source(Arc::new(error)))
            }
        }
    }

    fn require_settled_addition(&self, directory: &PrivateDirectory) -> Result<(), ApiError> {
        if let Some(operation) = read::<OperationId>(directory, &latest_name())? {
            let record: StoredAddition = read_record(directory, operation)?.ok_or_else(|| {
                ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "latest addition intent is missing",
                )
            })?;
            if record.status.state != AdditionState::Completed {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "continue the original pending addition before starting another plan",
                ));
            }
        }
        Ok(())
    }

    fn addition_status(
        &self,
        identity: &Identity,
        operation: Option<OperationId>,
    ) -> Result<AdditionStatusResponse, ApiError> {
        let directory =
            match PrivateDirectory::open_existing(self.inner.management_root.join(DIRECTORY)) {
                Ok(directory) => directory,
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    if operation.is_some() {
                        return Err(ApiError::new(
                            http::StatusCode::NOT_FOUND,
                            ErrorCode::OperationNotFound,
                            "storage addition was not found",
                        ));
                    }
                    return Ok(AdditionStatusResponse { operation: None });
                }
                Err(error) => return Err(storage_error(error)),
            };
        let operation = match operation {
            Some(operation) => Some(operation),
            None => read(&directory, &latest_name())?,
        };
        let Some(operation) = operation else {
            return Ok(AdditionStatusResponse { operation: None });
        };
        let mut record: StoredAddition = read_record(&directory, operation)?.ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                ErrorCode::OperationNotFound,
                "storage addition was not found",
            )
        })?;
        if record.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        let running = match self.inner.storage_addition.try_lock() {
            Ok(_guard) => false,
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        };
        if !running && record.status.state == AdditionState::Adding {
            record.status.state = AdditionState::RecoveryRequired;
        }
        Ok(AdditionStatusResponse {
            operation: Some(record.status),
        })
    }

    fn prepare_addition(
        &self,
        backend: &EngineBackend,
        identity: &Identity,
        request: AdditionRequest,
        discovery_root: &str,
    ) -> Result<
        (
            AdditionPreview,
            Config,
            Vec<storage_engine::PendingMemberDirectory>,
        ),
        ApiError,
    > {
        if request.additions.is_empty() || request.additions.len() > 32 {
            return Err(ApiError::invalid(
                "select between one and 32 storage directories",
            ));
        }
        let config = backend.addition_configuration(identity)?;
        let mut target = config.clone();
        let mut directories = Vec::with_capacity(request.additions.len());
        let mut excluded = vec![
            self.inner.data_root.clone(),
            self.inner.management_root.clone(),
        ];
        excluded.extend(config.disks.iter().map(|disk| disk.root.clone()));
        for addition in &request.additions {
            let held = crate::storage_directories::revalidate_within(
                discovery_root,
                &addition.selection,
                &excluded,
            )?;
            let root = PathBuf::from(&held.selection().root_path);
            excluded.push(root.clone());
            let id_hash = digest(&(request.operation_id, &addition.slot_id));
            let disk_id = format!("disk_{}", &id_hash[..32]);
            directories.push(storage_engine::PendingMemberDirectory {
                disk_id: disk_id.clone(),
                directory: held.into_storage_directory(),
            });
            target.disks.push(DiskConfig {
                slot_id: addition.slot_id.clone(),
                disk_id,
                root,
                level: addition.level,
                upstream_slot_id: addition.upstream_slot_id.clone(),
                metadata_member: addition.metadata_member,
                binding_generation: 1,
                capacity_bytes: None,
            });
        }
        storage_engine::ValidatedConfig::try_from(target.clone())
            .map_err(crate::engine_backend::error)?;
        let mut preview = AdditionPreview {
            pool_id: config.pool_id.clone(),
            configuration_digest: configuration_digest(&config),
            preview_digest: String::new(),
            original_metadata_members: config.disks.iter().filter(|d| d.metadata_member).count(),
            target_metadata_members: target.disks.iter().filter(|d| d.metadata_member).count(),
            added_directories: request.additions.len(),
            request,
        };
        preview.preview_digest = digest(&preview);
        Ok((preview, target, directories))
    }
}

/// The setup receipt stays immutable. A later configuration is admissible only
/// when a durable authorized addition binds it and retains the genesis settings.
/// Engine::open must still verify the complete membership authority and receipts.
pub(crate) fn admits_configuration(
    root: &Path,
    current: &Config,
    initial: &Config,
) -> Result<bool, ApiError> {
    let directory = match PrivateDirectory::open_existing(root.join(DIRECTORY)) {
        Ok(directory) => directory,
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(false);
        }
        Err(error) => return Err(storage_error(error)),
    };
    let Some(operation) = read::<OperationId>(&directory, &latest_name())? else {
        return Ok(false);
    };
    let record: StoredAddition = read_record(&directory, operation)?.ok_or_else(|| {
        ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "authorized addition intent is missing",
        )
    })?;
    let current_digest = configuration_digest(current);
    if current_digest != configuration_digest(&record.target)
        && current_digest != record.status.preview.configuration_digest
    {
        return Ok(false);
    }
    let mut original = current.clone();
    original
        .disks
        .retain(|disk| initial.disks.iter().any(|old| old.disk_id == disk.disk_id));
    Ok(configuration_digest(&original) == configuration_digest(initial))
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/storage_addition.rs"]
mod tests;
