//! Original manual-save requests and frozen bodies survive the live SMB FD.
use super::*;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AtomicFile, EntryName, PayloadOpenMode, PrivateDirectory};
use storage_contract::container_edit::DraftRevision;
use storage_contract::{ContentHash, Principal};

const RECORD_LIMIT: usize = 16 * 1024;

#[derive(Debug, error_definition::Error)]
pub enum RecoveryError {
    #[error("manual SMB recovery record is invalid: {0}")]
    Codec(#[source] json_codec::Error),
    #[error("manual SMB recovery record does not bind its original request")]
    Binding,
    #[error("manual SMB recovery body differs from its frozen request")]
    Body,
    #[error("manual SMB recovery exceeds its record or body budget")]
    Budget,
}
impl RecoveryError {
    fn api(self) -> ApiError {
        ApiError::conflict(storage_contract::ErrorCode::RecoveryRequired, &self.to_string())
            .with_source(crate::http_boundary::ApiErrorSource::ManualDraftRecovery(self))
    }
}

#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum Format {
    ManualDraftFrozenV1,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: Format,
    request: crate::PersistContainerDraft,
    // The persist boundary admits this as one private directory entry before
    // any open. It is never a caller-provided absolute path.
    live_name: PathBuf,
}

fn body_name(operation: OperationId) -> Result<EntryName, ApiError> {
    EntryName::new(format!("manual-{operation}")).map_err(crate::upload_fs_error)
}
fn record_name(operation: OperationId) -> Result<EntryName, ApiError> {
    EntryName::new(format!("manual-{operation}.work.json")).map_err(crate::upload_fs_error)
}
fn fingerprint(file: &File, limit: u64) -> Result<(ContentHash, u64), ApiError> {
    let mut hash = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut buffer = [0u8; 65_536];
    loop {
        let length = file.read_at(&mut buffer, offset).map_err(ApiError::io)?;
        if length == 0 {
            break;
        }
        offset = offset.checked_add(length as u64)
            .filter(|size| *size <= limit).ok_or_else(|| RecoveryError::Budget.api())?;
        hash.update(&buffer[..length]);
    }
    Ok((ContentHash::from_digest(hash.finalize()), offset))
}
fn decode(bytes: &[u8], operation: OperationId, principal: &str) -> Result<Record, ApiError> {
    let record: Record = json_codec::from_slice(bytes)
        .map_err(|error| RecoveryError::Codec(error).api())?;
    if record.request.revision.operation != operation
        || record.request.revision.principal.as_str() != principal
    {
        return Err(RecoveryError::Binding.api());
    }
    EntryName::new(&record.live_name).map_err(crate::upload_fs_error)?;
    Ok(record)
}
fn read_record(
    directory: &PrivateDirectory,
    operation: OperationId,
    principal: &str,
) -> Result<Record, ApiError> {
    let bytes = directory.read_bounded(&record_name(operation)?, RECORD_LIMIT)
        .map_err(crate::upload_fs_error)?;
    decode(&bytes, operation, principal)
}

/// Freeze once, before save publication. The private body is never lent to a
/// peer. A partial/occupied destination is retained and rejected, not replaced.
pub(super) fn freeze(
    shared: &Shared,
    identity: &Identity,
    writer: &Writer,
    limit: u64,
) -> Result<DraftRevision, ApiError> {
    let container = writer.container.as_ref()
        .expect("invariant: only admitted manual writers freeze manual requests");
    let original = writer.directory.read_bounded(&record_name(writer.operation_id)?, RECORD_LIMIT);
    match original {
        Ok(bytes) => {
            let record = decode(&bytes, writer.operation_id, &identity.principal)?;
            let revision = &record.request.revision;
            if record.request.context != container.context
                || record.request.entry.to_string() != writer.item_id
                || revision.base != container.base
                || revision.expected_generation != container.edit.generation()
                || record.live_name.as_os_str() != writer.source.file_name()
                    .ok_or_else(|| RecoveryError::Binding.api())?
            {
                return Err(RecoveryError::Binding.api());
            }
            return Ok(record.request.revision);
        }
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(crate::upload_fs_error(error)),
    }
    let name = body_name(writer.operation_id)?;
    let destination = writer.directory.path().join(name.as_path());
    let size = writer.file.metadata().map_err(ApiError::io)?.len();
    let _reservation = shared.budget.reserve(&identity.principal, &destination, size, limit)?;
    let mut frozen = writer.directory.open_payload(&name, PayloadOpenMode::CreateNew)
        .map_err(crate::upload_fs_error)?;
    let mut hash = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut buffer = [0u8; 65_536];
    loop {
        let count = writer.file.read_at(&mut buffer, offset).map_err(ApiError::io)?;
        if count == 0 {
            break;
        }
        offset = offset.checked_add(count as u64)
            .filter(|end| *end <= size).ok_or_else(|| RecoveryError::Body.api())?;
        frozen.write_all(&buffer[..count]).map_err(ApiError::io)?;
        hash.update(&buffer[..count]);
    }
    let content_hash = ContentHash::from_digest(hash.finalize());
    if offset != size || fingerprint(&writer.file, limit)? != (content_hash, size) {
        return Err(RecoveryError::Body.api());
    }
    frozen.sync_all().map_err(ApiError::io)?;
    writer.directory.verify_payload(&name, &frozen).map_err(crate::upload_fs_error)?;
    writer.directory.sync().map_err(crate::upload_fs_error)?;
    let revision = DraftRevision {
        operation: writer.operation_id,
        expected_generation: container.edit.generation(),
        principal: identity.principal.parse::<Principal>()
            .map_err(ApiError::invalid_authenticated_principal)?,
        base: container.base.clone(),
        content_hash,
        size,
    };
    let record = Record {
        format: Format::ManualDraftFrozenV1,
        request: crate::PersistContainerDraft {
            entry: writer.item_id.parse().map_err(ApiError::contract_input)?,
            context: container.context.clone(),
            revision: revision.clone(),
        },
        live_name: writer.source.file_name()
            .ok_or_else(|| RecoveryError::Binding.api())?.into(),
    };
    let bytes = json_codec::to_vec(&record)
        .map_err(|error| RecoveryError::Codec(error).api())?;
    if bytes.len() > RECORD_LIMIT {
        return Err(RecoveryError::Budget.api());
    }
    AtomicFile::create(&writer.directory, &record_name(writer.operation_id)?, &bytes)
        .map_err(crate::upload_fs_error)?;
    Ok(revision)
}

pub(super) fn save_frozen(
    shared: &Shared,
    identity: &Identity,
    operation: OperationId,
    limit: u64,
    resume: bool,
) -> Result<(RecordOutcome, bool), ApiError> {
    let directory = PrivateDirectory::open_trusted(&shared.spool)
        .map_err(crate::upload_fs_error)?;
    let record = read_record(&directory, operation, &identity.principal)?;
    let original = shared.backend.container_draft_save_receipt(identity, record.request.clone())?;
    let result = match original {
        Some(receipt) => RecordOutcome::Saved(receipt),
        None if !resume => RecordOutcome::Unresolved,
        None => {
            let name = body_name(operation)?;
            let body = directory.open_payload(&name, PayloadOpenMode::Existing)
                .map_err(crate::upload_fs_error)?;
            let revision = &record.request.revision;
            if fingerprint(&body, limit)? != (revision.content_hash, revision.size) {
                return Err(RecoveryError::Body.api());
            }
            directory.verify_payload(&name, &body).map_err(crate::upload_fs_error)?;
            // Persist performs current occurrence, original source, mode and
            // generation admission. No historical workspace commit is called.
            let saved = shared.backend.persist_container_draft(
                identity, record.request.clone(), &body,
            )?;
            RecordOutcome::Saved(saved.receipt)
        }
    };
    let live = directory.open_payload(&EntryName::new(&record.live_name)
        .map_err(crate::upload_fs_error)?, PayloadOpenMode::Existing);
    let changed = match live {
        Ok(live) => fingerprint(&live, limit)?
            != (record.request.revision.content_hash, record.request.revision.size),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => return Err(crate::upload_fs_error(error)),
    };
    Ok((result, changed))
}

pub(super) enum RecordOutcome {
    Saved(storage_contract::OperationResult),
    Unresolved,
}

pub(super) fn recover(
    shared: &Shared,
    identity: &Identity,
    operation: OperationId,
    limit: u64,
    resume: bool,
) -> Result<Value, ApiError> {
    let (outcome, live_changed) = save_frozen(shared, identity, operation, limit, resume)?;
    match outcome {
        RecordOutcome::Saved(receipt) => Ok(json!({
            "operation_id":operation,"draft_persisted":true,
            "original_receipt":receipt,"live_bytes_changed":live_changed,
        })),
        RecordOutcome::Unresolved => Ok(json!({
            "operation_id":operation,"draft_persisted":false,
            "live_bytes_changed":live_changed,
        })),
    }
}
