//! One reviewed new-primary workflow: retain source, publish registration, then
//! import to its empty root. The two durable phases are deliberately not atomic.
use super::*;
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use storage_addition::{read, storage_error, write};
use storage_contract::{
    CommitRequest, ErrorCode, OperationId,
    storage_addition::{
        AdditionCommit, AdditionContinuation, AdditionFailure, AdditionRequest, AdditionResume,
        AdditionState, DirectoryAddition,
    },
    storage_conversion::*,
};
use storage_engine::PreparedOrdinaryDirectory;

enum ConversionTarget<'a> {
    Fresh(&'a storage_contract::storage_directories::CandidateSelection),
    Retained(Option<&'a str>),
}
impl ConversionTarget<'_> {
    fn path(&self) -> Option<&str> {
        match self {
            Self::Fresh(selection) => Some(&selection.root_path),
            Self::Retained(path) => *path,
        }
    }
}

const DIRECTORY: &str = "ordinary-disk-conversions";
#[derive(data_serialization::Serialize, data_serialization::Deserialize)]
enum Format {
    #[serialization(rename = "ordinary-disk-conversion")]
    Ordinary,
}
#[derive(data_serialization::Serialize, data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: Format,
    principal: String,
    status: OrdinaryDiskConversionStatus,
    commit_request: Option<CommitRequest>,
    import_attempted: bool,
}

/// One admitted workflow owns its exact Engine throughout a possibly slow
/// source scan and registration. This is not serializable or a cached grant:
/// import revalidates current auth, target identity and core write fencing.
struct PreparedOrdinaryConversion {
    backend: Arc<EngineBackend>,
    source: PreparedOrdinaryDirectory,
}
fn name(id: OperationId) -> EntryName {
    EntryName::new(format!("{id}.json")).expect("invariant: operation identifier is a file name")
}
fn digest(preview: &OrdinaryDiskConversionPreview) -> String {
    let mut preview = preview.clone();
    preview.preview_digest.clear();
    storage_addition::digest(&preview)
}
fn distinct_directories(
    source: &storage_contract::storage_directories::DirectorySource,
    target: &storage_contract::storage_directories::CandidateSelection,
) -> Result<(), ApiError> {
    let source_path = Path::new(&source.path);
    let target_path = Path::new(&target.root_path);
    if source_path.starts_with(target_path)
        || target_path.starts_with(source_path)
        || (source.identity.device, source.identity.inode)
            == (target.root_identity.device, target.root_identity.inode)
    {
        return Err(ApiError::invalid(
            "ordinary conversion source and new target must be separate directories",
        ));
    }
    Ok(())
}
fn validate(preview: &OrdinaryDiskConversionPreview) -> Result<(), ApiError> {
    let [target] = preview.target.request.additions.as_slice() else {
        return Err(ApiError::invalid(
            "ordinary conversion requires one new primary target",
        ));
    };
    if target.level != 1
        || target.upstream_slot_id.is_some()
        || preview.target.authority.is_terminal()
        || preview.operation_id == preview.import_operation_id
        || preview.operation_id == preview.target.request.operation_id
        || preview.import_operation_id == preview.target.request.operation_id
        || digest(preview) != preview.preview_digest
    {
        return Err(ApiError::invalid(
            "ordinary conversion preview has an invalid role, identity or digest",
        ));
    }
    distinct_directories(&preview.source, &target.selection)?;
    preview
        .source_preview
        .digest
        .parse::<storage_engine::DirectoryConversionDigest>()
        .map_err(ApiError::invalid_source)?;
    Ok(())
}
fn source_preview(source: &PreparedOrdinaryConversion) -> OrdinarySourcePreview {
    let preview = source.source.preview();
    OrdinarySourcePreview {
        digest: preview.digest.to_string(),
        files: preview.files,
        directories: preview.directories,
        content_bytes: preview.content_bytes,
        temporary_bytes: preview.temporary_bytes,
    }
}
fn save(directory: &PrivateDirectory, record: &Record) -> Result<(), ApiError> {
    write(directory, &name(record.status.preview.operation_id), record)
}
fn read_record(directory: &PrivateDirectory, id: OperationId) -> Result<Record, ApiError> {
    let record: Record = read(directory, &name(id))?.ok_or_else(|| {
        ApiError::new(
            http::StatusCode::NOT_FOUND,
            ErrorCode::OperationNotFound,
            "ordinary conversion was not found",
        )
    })?;
    validate(&record.status.preview)?;
    if record.status.preview.operation_id != id {
        return Err(ApiError::invalid("conversion record identity differs"));
    }
    if let Some(request) = &record.commit_request {
        if request.operation_id != record.status.preview.import_operation_id
            || request.principal != record.principal
            || Some(request.workspace_id.as_str()) != record.status.workspace_id.as_deref()
        {
            return Err(ApiError::invalid(
                "conversion receipt does not bind its original workspace",
            ));
        }
    } else if record.status.result.is_some()
        || record.status.phase == OrdinaryDiskConversionPhase::Completed
    {
        return Err(ApiError::invalid(
            "completed conversion is missing its original workspace request",
        ));
    }
    if let Some(registration) = &record.status.registration
        && registration.preview != record.status.preview.target
    {
        return Err(ApiError::invalid(
            "conversion registration differs from its original review",
        ));
    }
    if record.status.stop_operation_id.is_some_and(|stop| {
        stop == record.status.preview.operation_id
            || stop == record.status.preview.import_operation_id
            || stop == record.status.preview.target.request.operation_id
    }) || record.status.stop_result.as_ref().is_some_and(|result| {
        Some(result.operation_id) != record.status.stop_operation_id
            || record.status.phase != OrdinaryDiskConversionPhase::Stopped
    }) || (matches!(
        record.status.phase,
        OrdinaryDiskConversionPhase::Stopping | OrdinaryDiskConversionPhase::Stopped
    ) && record.status.stop_operation_id.is_none())
        || (record.status.phase == OrdinaryDiskConversionPhase::Stopped
            && (record.status.result.is_some() || record.status.stop_result.is_none()))
        || (matches!(
            record.status.phase,
            OrdinaryDiskConversionPhase::Stopping | OrdinaryDiskConversionPhase::Stopped
        ) && !record
            .status
            .registration
            .as_ref()
            .is_some_and(|registration| {
                registration.state == AdditionState::Completed && registration.result.is_some()
            }))
    {
        return Err(ApiError::invalid(
            "conversion stop evidence does not bind its original intent",
        ));
    }
    Ok(record)
}
fn owned_record(
    directory: &PrivateDirectory,
    id: OperationId,
    identity: &Identity,
) -> Result<Record, ApiError> {
    let record = read_record(directory, id)?;
    if record.principal != identity.principal {
        return Err(ApiError::forbidden());
    }
    Ok(record)
}
fn stop_request(record: &Record) -> Result<storage_engine::DirectoryImportStopRequest, ApiError> {
    Ok(storage_engine::DirectoryImportStopRequest {
        principal: record.principal.clone(),
        target: EngineBackend::ordinary_conversion_identity(&record.status.preview.target)?,
        import_operation_id: record.status.preview.import_operation_id,
        root_operation_id: record.status.root_operation_id,
        workspace_id: record.status.workspace_id.clone(),
        commit_request: record.commit_request.clone(),
        stop_operation_id: record
            .status
            .stop_operation_id
            .ok_or_else(|| ApiError::invalid("conversion has no original stop intent"))?,
    })
}
fn apply_stop_result(
    record: &mut Record,
    outcome: storage_engine::DirectoryImportStopOutcome,
) -> Result<(), ApiError> {
    match outcome {
        storage_engine::DirectoryImportStopOutcome::Published(result) => {
            if record.status.stop_result.is_some()
                || record
                    .status
                    .result
                    .as_ref()
                    .is_some_and(|saved| saved != &result)
            {
                return Err(ApiError::conflict(
                    ErrorCode::ObjectCorrupt,
                    "saved conversion result differs from its permanent publication",
                ));
            }
            record.status.result = Some(result);
            record.status.stop_result = None;
            record.status.phase = OrdinaryDiskConversionPhase::Completed;
        }
        storage_engine::DirectoryImportStopOutcome::Stopped(result) => {
            if record.status.result.is_some()
                || record
                    .status
                    .stop_result
                    .as_ref()
                    .is_some_and(|saved| saved != &result)
            {
                return Err(ApiError::conflict(
                    ErrorCode::ObjectCorrupt,
                    "saved stop result differs from its permanent publication",
                ));
            }
            record.status.result = None;
            record.status.stop_result = Some(result);
            record.status.phase = OrdinaryDiskConversionPhase::Stopped;
        }
    }
    record.status.failure = None;
    Ok(())
}
impl ManagementBackend {
    pub(super) fn ordinary_conversion(
        &self,
        action: &str,
        input: Value,
        identity: &Identity,
    ) -> Result<Value, ApiError> {
        self.inner.current_management_identity(identity)?;
        if action == "ordinary_conversion_status" {
            #[derive(data_serialization::Deserialize)]
            #[serialization(deny_unknown_fields)]
            struct Query {
                operation_id: Option<OperationId>,
            }
            let query: Query = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            return self
                .ordinary_conversion_status(identity, query.operation_id)
                .map(|result| {
                    json_codec::to_value(result).expect("invariant: conversion status serializes")
                });
        }
        let _gate = self
            .inner
            .ordinary_conversion
            .try_lock()
            .map_err(|source| match source {
                std::sync::TryLockError::WouldBlock => ApiError::conflict(
                    ErrorCode::StorageRetryRunning,
                    "a conversion is running; query its original operation",
                ),
                std::sync::TryLockError::Poisoned(_) => ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "conversion coordination failed; restart and inspect the original operation",
                ),
            })?;
        match action {
            "ordinary_conversion_preview" => {
                let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let result = self.preview_ordinary_conversion(identity, request)?;
                Ok(json_codec::to_value(result).expect("invariant: conversion preview serializes"))
            }
            "ordinary_conversion_commit" => {
                let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let status = self.commit_ordinary_conversion(identity, request)?;
                Ok(json_codec::to_value(status).expect("invariant: conversion status serializes"))
            }
            "ordinary_conversion_resume" => {
                let request: OrdinaryDiskConversionResume =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let directory =
                    PrivateDirectory::open_existing(self.inner.management_root.join(DIRECTORY))
                        .map_err(storage_error)?;
                let record = owned_record(&directory, request.operation_id, identity)?;
                let status =
                    self.run_original_conversion(identity, &directory, record, None, true)?;
                Ok(json_codec::to_value(status).expect("invariant: conversion status serializes"))
            }
            "ordinary_conversion_stop" => {
                let request: OrdinaryDiskConversionStop =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let status = self.stop_ordinary_conversion(identity, request)?;
                Ok(json_codec::to_value(status)
                    .expect("invariant: conversion stop status serializes"))
            }
            _ => Err(ApiError::invalid("unknown ordinary conversion action")),
        }
    }

    fn stop_ordinary_conversion(
        &self,
        identity: &Identity,
        request: OrdinaryDiskConversionStop,
    ) -> Result<OrdinaryDiskConversionStatus, ApiError> {
        if !request.acknowledge_retained_state {
            return Err(ApiError::invalid(
                "confirm that stopping retains the source, admitted disk and staging evidence",
            ));
        }
        let directory = PrivateDirectory::open_existing(self.inner.management_root.join(DIRECTORY))
            .map_err(storage_error)?;
        let mut record = owned_record(&directory, request.operation_id, identity)?;
        if self.reconcile_conversion(identity, &mut record)? {
            return Ok(record.status);
        }
        if !record
            .status
            .registration
            .as_ref()
            .is_some_and(|registration| {
                registration.state == AdditionState::Completed && registration.result.is_some()
            })
        {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "settle the original disk registration before stopping its content import",
            ));
        }
        if record.status.stop_operation_id.is_none() {
            record.status.stop_operation_id =
                Some(OperationId::generate().map_err(ApiError::contract_input)?);
        }
        record.status.phase = OrdinaryDiskConversionPhase::Stopping;
        record.status.failure = None;
        save(&directory, &record)?;
        let backend = self.inner.addition_backend()?;
        match backend.stop_ordinary_conversion(identity, &stop_request(&record)?) {
            Ok(result) => apply_stop_result(&mut record, result)?,
            Err(error) => {
                record.status.phase = OrdinaryDiskConversionPhase::RecoveryRequired;
                record.status.failure = Some(AdditionFailure {
                    code: error.code,
                    message: error.message.clone(),
                });
                save(&directory, &record)?;
                return Err(error);
            }
        }
        save(&directory, &record)?;
        Ok(record.status)
    }

    fn commit_ordinary_conversion(
        &self,
        identity: &Identity,
        request: OrdinaryDiskConversionCommit,
    ) -> Result<OrdinaryDiskConversionStatus, ApiError> {
        if !request.acknowledge_contents_only {
            return Err(ApiError::invalid(
                "confirm contents-only scope before converting",
            ));
        }
        validate(&request.preview)?;
        let parent =
            PrivateDirectory::open_existing(&self.inner.management_root).map_err(storage_error)?;
        let directory = parent
            .create_child(&EntryName::new(DIRECTORY).expect("invariant: static control directory"))
            .map_err(storage_error)?;
        let operation = request.preview.operation_id;
        if read::<Record>(&directory, &name(operation))?.is_some() {
            let record = owned_record(&directory, operation, identity)?;
            if record.status.preview != request.preview {
                return Err(ApiError::conflict(
                    ErrorCode::IdempotencyKeyReused,
                    "conversion operation belongs to another reviewed request",
                ));
            }
            return self.run_original_conversion(identity, &directory, record, None, false);
        }
        self.ensure_prior_conversion_settled(&directory, identity)?;
        // Complete the source scan and target review before any target writes.
        let source = self.inspect_conversion_input(
            identity,
            &request.preview.source,
            ConversionTarget::Fresh(&request.preview.target.request.additions[0].selection),
        )?;
        if source_preview(&source) != request.preview.source_preview {
            return Err(ApiError::conflict(
                ErrorCode::PlanStale,
                "source changed after conversion review",
            ));
        }
        let actual =
            self.preview_storage_addition(identity, request.preview.target.request.clone())?;
        if actual != request.preview.target {
            return Err(ApiError::conflict(
                ErrorCode::PlanStale,
                "target changed after conversion review",
            ));
        }
        let record = Record {
            format: Format::Ordinary,
            principal: identity.principal.clone(),
            status: OrdinaryDiskConversionStatus {
                preview: request.preview,
                phase: OrdinaryDiskConversionPhase::Registering,
                registration: None,
                workspace_id: None,
                root_operation_id: None,
                result: None,
                stop_operation_id: None,
                stop_result: None,
                failure: None,
            },
            commit_request: None,
            import_attempted: false,
        };
        save(&directory, &record)?;
        write(&directory, &storage_addition::latest_name(), &operation)?;
        self.run_original_conversion(identity, &directory, record, Some(source), false)
    }

    fn run_original_conversion(
        &self,
        identity: &Identity,
        directory: &PrivateDirectory,
        mut record: Record,
        prepared: Option<PreparedOrdinaryConversion>,
        resume: bool,
    ) -> Result<OrdinaryDiskConversionStatus, ApiError> {
        if self.reconcile_conversion(identity, &mut record)? {
            return Ok(record.status);
        }
        if record.status.stop_operation_id.is_some() {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "continue the original stop operation; import cannot restart after a stop intent",
            ));
        }
        if record.commit_request.is_some()
            || record.status.workspace_id.is_some()
            || record.status.root_operation_id.is_some()
        {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "continue the retained import workspace or original operation; conversion never creates a replacement workspace after an uncertain import",
            ));
        }
        if record.import_attempted && !resume {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "explicitly resume the original conversion; no import write intent was recorded",
            ));
        }
        let source = match prepared {
            Some(source) => source,
            None => self.inspect_conversion_source(
                identity,
                &record.status.preview.source,
                Some(
                    &record.status.preview.target.request.additions[0]
                        .selection
                        .root_path,
                ),
            )?,
        };
        if source_preview(&source) != record.status.preview.source_preview {
            return Err(ApiError::conflict(
                ErrorCode::PlanStale,
                "source differs from the original conversion",
            ));
        }
        if let Err(error) =
            self.execute_ordinary_conversion(identity, directory, &mut record, source, resume)
        {
            record.status.phase = OrdinaryDiskConversionPhase::RecoveryRequired;
            record.status.failure = Some(AdditionFailure {
                code: error.code,
                message: error.message.clone(),
            });
            save(directory, &record)?;
            return Err(error);
        }
        Ok(record.status)
    }

    fn inspect_conversion_source(
        &self,
        identity: &Identity,
        source: &storage_contract::storage_directories::DirectorySource,
        target: Option<&str>,
    ) -> Result<PreparedOrdinaryConversion, ApiError> {
        self.inspect_conversion_input(identity, source, ConversionTarget::Retained(target))
    }

    fn inspect_conversion_input(
        &self,
        identity: &Identity,
        source: &storage_contract::storage_directories::DirectorySource,
        target: ConversionTarget<'_>,
    ) -> Result<PreparedOrdinaryConversion, ApiError> {
        let backend = self.inner.backend()?;
        let (config, stopped) = backend.addition_directory_scope(identity)?;
        let mut excluded = vec![
            self.inner.data_root.clone(),
            self.inner.management_root.clone(),
        ];
        excluded.extend(config.disks.into_iter().map(|disk| disk.root));
        let target_directory = match &target {
            ConversionTarget::Fresh(selection) => {
                let mut target_excluded = excluded.clone();
                target_excluded.push(PathBuf::from(&source.path));
                Some(
                    crate::storage_directories::revalidate_fresh_within(
                        self.inner.discovery_root.as_str(),
                        selection,
                        &target_excluded,
                        &stopped,
                    )?
                    .into_storage_directory(),
                )
            }
            // Retained registration is checked against its exact original
            // receipt by run_ordinary_conversion. Never reopen it as empty.
            ConversionTarget::Retained(_) => None,
        };
        excluded.extend(target.path().map(PathBuf::from));
        let directory = crate::storage_directories::reopen_source(
            self.inner.discovery_root.as_str(),
            source,
            &excluded,
            &stopped,
        )?;
        let source = match target_directory {
            Some(target) => PreparedOrdinaryDirectory::inspect_for_target(
                directory,
                Path::new(&source.path),
                &target,
            ),
            None => PreparedOrdinaryDirectory::inspect(directory, Path::new(&source.path)),
        }
        .map_err(crate::engine_backend::error)?;
        Ok(PreparedOrdinaryConversion { backend, source })
    }
    fn preview_ordinary_conversion(
        &self,
        identity: &Identity,
        request: OrdinaryDiskConversionRequest,
    ) -> Result<OrdinaryDiskConversionPreview, ApiError> {
        name_policy::validate_slot(&request.slot_id).map_err(ApiError::invalid_source)?;
        distinct_directories(&request.source, &request.target)?;
        let source = self.inspect_conversion_input(
            identity,
            &request.source,
            ConversionTarget::Fresh(&request.target),
        )?;
        let target = self.preview_storage_addition(
            identity,
            AdditionRequest {
                operation_id: OperationId::generate().map_err(ApiError::contract_input)?,
                additions: vec![DirectoryAddition {
                    selection: request.target,
                    slot_id: request.slot_id,
                    level: 1,
                    upstream_slot_id: None,
                    metadata_member: request.metadata_member,
                }],
            },
        )?;
        let mut preview = OrdinaryDiskConversionPreview {
            operation_id: request.operation_id,
            import_operation_id: OperationId::generate().map_err(ApiError::contract_input)?,
            source: request.source,
            source_preview: source_preview(&source),
            target,
            preview_digest: String::new(),
        };
        preview.preview_digest = digest(&preview);
        validate(&preview)?;
        Ok(preview)
    }
    fn execute_ordinary_conversion(
        &self,
        identity: &Identity,
        directory: &PrivateDirectory,
        record: &mut Record,
        prepared: PreparedOrdinaryConversion,
        resume: bool,
    ) -> Result<(), ApiError> {
        // The source scan and registration may outlive a cached health sample.
        // Preserve their original admitted owner instead of redispatching or
        // substituting an unchecked recovery backend.
        let PreparedOrdinaryConversion { backend, source } = prepared;
        let target = record.status.preview.target.clone();
        let registration = if resume && record.status.registration.is_some() {
            self.resume_storage_addition(
                identity,
                AdditionResume {
                    operation_id: target.request.operation_id,
                    continuation: AdditionContinuation::Add,
                },
            )?
        } else {
            self.commit_storage_addition(
                identity,
                AdditionCommit {
                    preview: target.clone(),
                },
            )?
        };
        if registration.state != AdditionState::Completed || registration.result.is_none() {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "new primary registration is not permanently complete",
            ));
        }
        record.status.registration = Some(registration);
        record.status.phase = OrdinaryDiskConversionPhase::Importing;
        record.import_attempted = true;
        record.status.failure = None;
        save(directory, record)?;
        self.inner.ensure_running()?;
        let result = backend
            .run_ordinary_conversion(
                identity,
                &target,
                source,
                record.status.preview.import_operation_id,
                |progress| {
                    record.status.workspace_id = progress.workspace_id().map(str::to_owned);
                    record.status.root_operation_id = progress.root_operation_id();
                    record.commit_request = progress.request().cloned();
                    save(directory, record).map_err(|source| {
                        storage_engine::Error::from(std::io::Error::other(source))
                    })
                },
            )
            .map_err(|failure| {
                if failure.before_import() {
                    record.import_attempted = false;
                }
                failure.into_api()
            })?;
        if result.operation_id != record.status.preview.import_operation_id
            || result.kind != "workspace.commit"
            || !matches!(
                result.commit_state,
                storage_contract::CommitState::Published
                    | storage_contract::CommitState::Acked
                    | storage_contract::CommitState::PublishedDegraded
            )
        {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "conversion import has no exact published result",
            ));
        }
        record.status.result = Some(result);
        record.status.phase = OrdinaryDiskConversionPhase::Completed;
        save(directory, record)
    }
    fn reconcile_conversion(
        &self,
        identity: &Identity,
        record: &mut Record,
    ) -> Result<bool, ApiError> {
        self.reconcile_conversion_registration(identity, record)?;
        if record.status.stop_operation_id.is_some() {
            let backend = self.inner.addition_backend()?;
            if let Some(outcome) =
                backend.ordinary_conversion_stop_result(identity, &stop_request(record)?)?
            {
                apply_stop_result(record, outcome)?;
                return Ok(true);
            }
            if record.status.stop_result.is_some() {
                return Err(ApiError::conflict(
                    ErrorCode::ObjectCorrupt,
                    "saved stop receipt has no exact permanent publication",
                ));
            }
            return Ok(false);
        }
        let Some(request) = &record.commit_request else {
            if record.status.phase == OrdinaryDiskConversionPhase::Completed {
                return Err(ApiError::invalid(
                    "completed conversion lacks its original commit request",
                ));
            }
            return Ok(false);
        };
        let backend = self.inner.addition_backend()?;
        if let Some(result) = backend.ordinary_conversion_result(identity, request)? {
            if record
                .status
                .result
                .as_ref()
                .is_some_and(|saved| saved != &result)
            {
                return Err(ApiError::conflict(
                    ErrorCode::ObjectCorrupt,
                    "saved conversion receipt differs from permanent publication",
                ));
            }
            record.status.result = Some(result);
            record.status.phase = OrdinaryDiskConversionPhase::Completed;
            record.status.failure = None;
            return Ok(true);
        }
        if record.status.result.is_some() {
            return Err(ApiError::conflict(
                ErrorCode::ObjectCorrupt,
                "saved conversion receipt has no exact permanent publication",
            ));
        }
        Ok(false)
    }

    fn reconcile_conversion_registration(
        &self,
        identity: &Identity,
        record: &mut Record,
    ) -> Result<(), ApiError> {
        if record.status.registration.as_ref().is_some_and(|status| {
            status.state == AdditionState::Completed && status.result.is_some()
        }) {
            return Ok(());
        }
        let directory = match PrivateDirectory::open_existing(
            self.inner.management_root.join(storage_addition::DIRECTORY),
        ) {
            Ok(directory) => directory,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(error) => return Err(storage_error(error)),
        };
        let Some(mut addition) = storage_addition::read_record(
            &directory,
            record.status.preview.target.request.operation_id,
        )?
        else {
            return Ok(());
        };
        if addition.principal != record.principal
            || addition.status.preview != record.status.preview.target
        {
            return Err(ApiError::conflict(
                ErrorCode::ObjectCorrupt,
                "conversion registration evidence differs from its original intent",
            ));
        }
        if let Some(result) = self
            .inner
            .addition_backend()?
            .storage_addition_result(identity, &addition)?
        {
            storage_addition::finish(&mut addition, result);
            record.status.registration = Some(addition.status);
        }
        Ok(())
    }
    fn ensure_prior_conversion_settled(
        &self,
        directory: &PrivateDirectory,
        identity: &Identity,
    ) -> Result<(), ApiError> {
        if let Some(id) = read::<OperationId>(directory, &storage_addition::latest_name())? {
            // Completion gates all future conversions, but is not authority to
            // disclose another principal's source, preview or receipt. A
            // recovered administrator may proceed only after the exact original
            // publication is proven, never by taking over unfinished work.
            let record = read_record(directory, id)?;
            if !self.conversion_record_settled(identity, &record)? {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "finish the original conversion before starting another",
                ));
            }
        }
        Ok(())
    }
    fn conversion_record_settled(
        &self,
        identity: &Identity,
        record: &Record,
    ) -> Result<bool, ApiError> {
        if record.status.stop_operation_id.is_some() {
            self.inner
                .addition_backend()?
                .ordinary_conversion_stop_settled(identity, &stop_request(record)?)
        } else {
            match &record.commit_request {
                Some(request) => self
                    .inner
                    .addition_backend()?
                    .ordinary_conversion_settled(identity, request),
                None => Ok(false),
            }
        }
    }
    fn ordinary_conversion_status(
        &self,
        identity: &Identity,
        id: Option<OperationId>,
    ) -> Result<OrdinaryDiskConversionStatusResponse, ApiError> {
        // Keep an available gate until the record has been read and reconciled.
        // Reading before acquiring it could turn a stale pre-write snapshot into
        // RecoveryRequired immediately after the writer actually completed.
        let gate = match self.inner.ordinary_conversion.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "conversion coordination failed; restart and inspect the original operation",
                ));
            }
        };
        let directory =
            match PrivateDirectory::open_existing(self.inner.management_root.join(DIRECTORY)) {
                Ok(directory) => directory,
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    if id.is_some() {
                        return Err(ApiError::new(
                            http::StatusCode::NOT_FOUND,
                            ErrorCode::OperationNotFound,
                            "ordinary conversion was not found",
                        ));
                    }
                    return Ok(OrdinaryDiskConversionStatusResponse { operation: None });
                }
                Err(error) => return Err(storage_error(error)),
            };
        let explicit = id.is_some();
        let selected = match id {
            Some(id) => Some(id),
            None => read(&directory, &storage_addition::latest_name())?,
        };
        let Some(id) = selected else {
            return Ok(OrdinaryDiskConversionStatusResponse { operation: None });
        };
        let mut record = read_record(&directory, id)?;
        if record.principal != identity.principal {
            if !explicit && self.conversion_record_settled(identity, &record)? {
                // Global latest is only a startup hint. Do not expose the prior
                // owner's completed workflow to a recovered administrator.
                return Ok(OrdinaryDiskConversionStatusResponse { operation: None });
            }
            return Err(ApiError::forbidden());
        }
        // A busy writer may expose progress, but cannot make a cached terminal
        // projection authoritative. Reconcile terminal evidence read-only even
        // while another workflow owns the gate; no control record is saved here.
        let terminal = matches!(
            record.status.phase,
            OrdinaryDiskConversionPhase::Completed | OrdinaryDiskConversionPhase::Stopped
        );
        if (gate.is_some() || terminal) && !self.reconcile_conversion(identity, &mut record)? {
            record.status.phase = OrdinaryDiskConversionPhase::RecoveryRequired;
        }
        Ok(OrdinaryDiskConversionStatusResponse {
            operation: Some(record.status),
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/ordinary_conversion.rs"]
mod tests;
