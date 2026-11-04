//! Durable control-plane intent; engine receipts remain publication authority.
use super::*;
use data_serialization::{Deserialize, Serialize, de::DeserializeOwned};
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory};
use storage_contract::{Config, DiskConfig, ErrorCode, OperationId, storage_addition::*};

pub(super) const DIRECTORY: &str = "storage-additions";
const MAX_RECORD: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Format {
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
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AdditionSource {
    ActiveMembership,
    TerminalManagement { original: Config },
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct StoredAddition {
    pub(super) source: AdditionSource,
    pub(super) format: Format,
    pub(super) principal: String,
    pub(crate) target: Config,
    pub(crate) archive: Option<ArchiveBinding>,
    pub(crate) status: AdditionStatus,
}

pub(crate) fn configuration_digest(config: &Config) -> String {
    digest(config)
}
pub(crate) fn digest(value: &impl Serialize) -> String {
    let bytes = json_codec::to_vec(value).expect("invariant: typed addition serializes");
    blake3::hash(&bytes).to_hex().to_string()
}
pub(super) fn storage_error(source: platform_io::fs_safety::Error) -> ApiError {
    ApiError::new(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::IoError,
        "storage lifecycle control record could not be read or synchronized",
    )
    .with_source(source)
}
pub(super) fn name(operation: OperationId) -> EntryName {
    EntryName::new(format!("{operation}.json")).expect("invariant: typed operation is a filename")
}
pub(super) fn read<T: DeserializeOwned>(
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
pub(super) fn read_record(
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
        || !valid_status(&record.status, record.archive.as_ref())
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
            || disk.root.as_path() != Path::new(&addition.selection.root_path)
            || disk.level != addition.level
            || disk.upstream_slot_id != addition.upstream_slot_id
            || disk.metadata_member != addition.metadata_member
            || disk.binding_generation != 1
            || disk.capacity_bytes.is_some()
        {
            return Err(corrupt());
        }
    }
    match (&record.source, &preview.authority) {
        (AdditionSource::ActiveMembership, AdditionAuthority::ActiveMembership) => {}
        (
            AdditionSource::TerminalManagement { original: saved },
            AdditionAuthority::TerminalManagement {
                terminal_authority_hash,
                ..
            },
        ) => {
            let mut historical = saved.clone();
            for disk in &mut historical.disks {
                disk.metadata_member = false;
            }
            if configuration_digest(&original) != configuration_digest(&historical)
                || terminal_authority_hash.len() != 64
                || !terminal_authority_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || preview
                    .request
                    .additions
                    .iter()
                    .any(|add| !add.metadata_member)
            {
                return Err(corrupt());
            }
            storage_engine::ValidatedConfig::try_from(saved.clone())
                .map_err(crate::engine_backend::error)?;
            original = saved.clone();
        }
        _ => return Err(corrupt()),
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

fn running_state(continuation: AdditionContinuation) -> AdditionState {
    match continuation {
        AdditionContinuation::Add => AdditionState::Adding,
        AdditionContinuation::Cancel => AdditionState::Cancelling,
    }
}

pub(super) fn finish(record: &mut StoredAddition, result: storage_contract::OperationResult) {
    let cancelled = result.kind == "membership-add-cancelled";
    record.status.state = if cancelled {
        AdditionState::Cancelled
    } else {
        AdditionState::Completed
    };
    record.status.phase = if cancelled {
        AdditionPhase::Cancelled
    } else {
        AdditionPhase::Completed
    };
    record.status.continuation = if cancelled {
        AdditionContinuation::Cancel
    } else {
        AdditionContinuation::Add
    };
    record.status.result = Some(result);
    record.status.failure = None;
}

fn valid_status(status: &AdditionStatus, archive: Option<&ArchiveBinding>) -> bool {
    if (status.preview.authority.is_terminal()
        && (status.continuation != AdditionContinuation::Add
            || matches!(
                status.state,
                AdditionState::Cancelling | AdditionState::Cancelled
            )
            || !status.phase.is_metadata_resume()))
        || (status.state == AdditionState::Completed) != (status.phase == AdditionPhase::Completed)
        || (status.state == AdditionState::Cancelled) != (status.phase == AdditionPhase::Cancelled)
        || status.state.is_terminal() != status.result.is_some()
        || (status.state.is_terminal() && status.failure.is_some())
    {
        return false;
    }
    status.result.as_ref().is_none_or(|result| {
        result.operation_id == status.operation_id
            && result.commit_state == storage_contract::CommitState::Published
            && result.kind
                == if status.preview.authority.is_terminal() {
                    "terminal-metadata-resume"
                } else if status.state == AdditionState::Cancelled {
                    "membership-add-cancelled"
                } else {
                    "membership-add"
                }
            && archive.is_some_and(|binding| result.request_digest == binding.transition_hash)
            && match &status.preview.authority {
                AdditionAuthority::ActiveMembership => true,
                AdditionAuthority::TerminalManagement {
                    terminal_authority_hash,
                    ..
                } => {
                    result
                        .result
                        .get("terminal_authority")
                        .and_then(Value::as_str)
                        == Some(terminal_authority_hash.as_str())
                        && result
                            .result
                            .get("metadata_members")
                            .and_then(Value::as_u64)
                            == Some(status.preview.target_metadata_members as u64)
                }
            }
    })
}

pub(super) fn write(
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
pub(super) fn latest_name() -> EntryName {
    EntryName::new("latest.json").expect("invariant: static addition pointer")
}
pub(super) fn save(directory: &PrivateDirectory, record: &StoredAddition) -> Result<(), ApiError> {
    write(directory, &name(record.status.operation_id), record)
}
fn conflict(message: &'static str) -> ApiError {
    ApiError::conflict(ErrorCode::IdempotencyKeyReused, message)
}

impl Inner {
    pub(super) fn addition_backend(&self) -> Result<Arc<EngineBackend>, ApiError> {
        self.ensure_running()?;
        // A fenced pool is deliberately not ready for ordinary business writes.
        // Retain its exact engine for authorized recovery of this operation.
        crate::lock(&self.state)
            .backend
            .engine()
            .cloned()
            .ok_or_else(|| {
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
        self.inner.current_management_identity(identity)?;
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
        let backend = crate::lock(&self.inner.state).backend.engine().cloned();
        if action == "disk_add_preview" {
            self.require_settled_exit()?;
            let request: AdditionRequest =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            let (preview, _, _) = self.prepare_addition(
                backend.as_deref(),
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
                self.require_settled_exit()?;
                self.require_settled_addition(&directory)?;
                let (preview, target, directories) = self.prepare_addition(
                    backend.as_deref(),
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
                let source = if preview.authority.is_terminal() {
                    AdditionSource::TerminalManagement {
                        original: self.inner.terminal_configuration(identity)?.0,
                    }
                } else {
                    AdditionSource::ActiveMembership
                };
                let record = StoredAddition {
                    source,
                    format: Format::Addition,
                    principal: identity.principal.clone(),
                    target,
                    archive: None,
                    status: AdditionStatus {
                        operation_id: operation,
                        state: AdditionState::Adding,
                        phase: AdditionPhase::Prepared,
                        continuation: AdditionContinuation::Add,
                        result: None,
                        preview,
                        failure: None,
                    },
                };
                save(&directory, &record)?;
                record
            }
        } else if matches!(action, "disk_add_resume" | "disk_add_cancel") {
            let (operation, continuation, preview_digest) = if action == "disk_add_cancel" {
                let request: AdditionCancel =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                (
                    request.operation_id,
                    AdditionContinuation::Cancel,
                    Some(request.preview_digest),
                )
            } else {
                let request: AdditionResume =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                (request.operation_id, request.continuation, None)
            };
            let mut saved = read_record(&directory, operation)?.ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    ErrorCode::OperationNotFound,
                    "storage addition was not found",
                )
            })?;
            if preview_digest.is_some_and(|digest| digest != saved.status.preview.preview_digest) {
                return Err(conflict(
                    "cancellation does not bind the original addition preview",
                ));
            }
            if saved.status.preview.authority.is_terminal()
                && continuation == AdditionContinuation::Cancel
            {
                return Err(ApiError::new(
                    http::StatusCode::NOT_IMPLEMENTED,
                    ErrorCode::UnsupportedOperation,
                    "cancellation is not implemented for this metadata-resume stage; continue the original operation",
                ));
            }
            if !saved.status.state.is_terminal() {
                saved.status.continuation = continuation;
            }
            saved
        } else {
            return Err(ApiError::invalid("unknown storage addition action"));
        };
        if record.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        if record.status.preview.authority.is_terminal() {
            return self.execute_terminal_resume(identity, &directory, record, held_directories);
        }
        let backend = backend.ok_or_else(|| {
            ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "the original addition Engine is unavailable",
            )
        })?;
        // Even exact terminal replay requires a currently valid system session.
        backend.addition_configuration(identity)?;
        if record.status.state.is_terminal() {
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
        if let Some(result) = backend.storage_addition_result(identity, &record)? {
            finish(&mut record, result);
            save(&directory, &record)?;
            return Ok(
                json_codec::to_value(record.status).expect("invariant: addition status serializes")
            );
        }
        // Save the selected continuation before any irreversible shared-slot vote.
        // Retrying an old commit body retains this choice rather than undoing Cancel.
        record.status.state = running_state(record.status.continuation);
        record.status.failure = None;
        save(&directory, &record)?;
        write(&directory, &latest_name(), &record.status.operation_id)?;
        let intent = record.clone();
        let outcome =
            backend.run_storage_addition(identity, &intent, held_directories, |phase, archive| {
                if let Some(archive) = archive {
                    record.archive = Some(archive);
                }
                record.status.phase = phase;
                record.status.state = running_state(record.status.continuation);
                record.status.failure = None;
                save(&directory, &record)
            });
        match outcome {
            Ok(result) => {
                finish(&mut record, result);
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

    pub(super) fn require_no_pending_addition(&self) -> Result<(), ApiError> {
        let directory =
            match PrivateDirectory::open_existing(self.inner.management_root.join(DIRECTORY)) {
                Ok(directory) => directory,
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(());
                }
                Err(error) => return Err(storage_error(error)),
            };
        self.require_settled_addition(&directory)
    }

    pub(super) fn require_settled_addition(
        &self,
        directory: &PrivateDirectory,
    ) -> Result<(), ApiError> {
        if let Some(operation) = read::<OperationId>(directory, &latest_name())? {
            let record: StoredAddition = read_record(directory, operation)?.ok_or_else(|| {
                ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "latest addition intent is missing",
                )
            })?;
            if !record.status.state.is_terminal() {
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
        if !running && record.status.preview.authority.is_terminal() {
            if let Some(result) = self.inner.terminal_resume_result(identity, &record)? {
                finish(&mut record, result);
            } else {
                record.status.result = None;
                record.status.state = AdditionState::RecoveryRequired;
                if record.status.phase == AdditionPhase::Completed {
                    record.status.phase = AdditionPhase::Activating;
                }
            }
        } else if !running && !record.status.state.is_terminal() {
            let backend = crate::lock(&self.inner.state).backend.engine().cloned();
            if let Some(backend) = backend
                && let Some(result) = backend.storage_addition_result(identity, &record)?
            {
                finish(&mut record, result);
            } else {
                record.status.state = AdditionState::RecoveryRequired;
            }
        }
        Ok(AdditionStatusResponse {
            operation: Some(record.status),
        })
    }

    pub(super) fn prepare_addition(
        &self,
        backend: Option<&EngineBackend>,
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
        let (config, authority, stopped) = match backend {
            Some(backend) => {
                let (config, stopped) = backend.addition_directory_scope(identity)?;
                (config, AdditionAuthority::ActiveMembership, stopped)
            }
            None => {
                let terminal = self.inner.terminal(identity)?.ok_or_else(|| {
                    ApiError::conflict(
                        ErrorCode::RecoveryRequired,
                        "continue the existing metadata resume before selecting another group",
                    )
                })?;
                if terminal.operation(request.operation_id).is_some() {
                    return Err(conflict(
                        "metadata resume operation_id already belongs to permanent history",
                    ));
                }
                let (config, authority) = self.inner.terminal_configuration(identity)?;
                if request.additions.iter().any(|add| !add.metadata_member) {
                    return Err(ApiError::invalid(
                        "metadata resume requires fresh metadata directories",
                    ));
                }
                let stopped = config.disks.iter().map(|disk| disk.root.clone()).collect();
                (config, authority, stopped)
            }
        };
        let mut target = config.clone();
        if authority.is_terminal() {
            for disk in &mut target.disks {
                disk.metadata_member = false;
            }
        }
        let mut directories = Vec::with_capacity(request.additions.len());
        let mut excluded = vec![
            self.inner.data_root.clone(),
            self.inner.management_root.clone(),
        ];
        excluded.extend(
            config
                .disks
                .iter()
                .filter(|disk| !stopped.contains(&disk.root))
                .map(|disk| disk.root.clone()),
        );
        for addition in &request.additions {
            let held = crate::storage_directories::revalidate_fresh_within(
                discovery_root,
                &addition.selection,
                &excluded,
                &stopped,
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
            authority,
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

pub(super) fn configuration_edges(
    root: &Path,
    chain: &mut super::storage_configuration::ConfigurationChain,
) -> Result<(), ApiError> {
    super::storage_configuration::records(&root.join(DIRECTORY), read_record, |record, latest| {
        let original = record.status.preview.configuration_digest;
        let target = configuration_digest(&record.target);
        if latest {
            chain.tip(original.clone());
            chain.tip(target.clone());
        }
        chain.edge(original, target);
        Ok(())
    })
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/storage_addition.rs"]
mod tests;
