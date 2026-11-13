//! Typed manual working material before the first complete frozen request exists.
use super::*;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    AdvisoryLock, AtomicFile, EntryName, PayloadOpenMode, PrivateDirectory,
};
use storage_contract::container_edit::{DraftBase, DraftRevision};
use storage_contract::{EntryId, Generation, Principal};
use manual_recovery::RecoveryError;

const RECORD_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Origin {
    pub(super) context: crate::ReadContainerEdit,
    pub(super) entry: EntryId,
    pub(super) base: DraftBase,
    pub(super) expected_generation: Generation,
    pub(super) operation: OperationId,
    pub(super) principal: Principal,
    pub(super) live_name: PathBuf,
}

#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "smb-manual-working-before-save-v1")]
    ManualWorking,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Phase {
    Working,
    Frozen { revision: DraftRevision },
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum HandleState {
    Active,
    Detached { code: storage_contract::ErrorCode, message: String, bytes_synced: bool },
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: Format,
    origin: Origin,
    phase: Phase,
    handle: HandleState,
}

pub(super) fn origin(writer: &Writer, identity: &str) -> Result<Origin, ApiError> {
    let container = writer.container.as_ref()
        .expect("invariant: only a manual container creates manual working material");
    Ok(Origin {
        context: container.context.clone(),
        entry: writer.item_id.parse().map_err(ApiError::contract_input)?,
        base: container.base.clone(),
        expected_generation: container.revision.as_ref()
            .map_or(container.edit.generation(), |revision| revision.expected_generation),
        operation: writer.operation_id,
        principal: identity.parse().map_err(ApiError::invalid_authenticated_principal)?,
        live_name: writer.source.file_name()
            .ok_or_else(|| RecoveryError::Binding.api())?.into(),
    })
}

fn record_lock(directory: &PrivateDirectory, name: &EntryName) -> Result<AdvisoryLock, ApiError> {
    let lock = EntryName::new(name.as_path().with_extension("lock"))
        .map_err(crate::upload_fs_error)?;
    AdvisoryLock::acquire(directory, &lock.as_relative()).map_err(crate::upload_fs_error)
}

fn persist_reference(
    directory: &PrivateDirectory,
    locator: &EntryName,
    record: &mut Record,
    revision: DraftRevision,
) -> Result<(), ApiError> {
    record.phase = Phase::Frozen { revision };
    let bytes = json_codec::to_vec(record)
        .map_err(|error| RecoveryError::Codec(error).api())?;
    if bytes.len() > RECORD_LIMIT {
        return Err(RecoveryError::Budget.api());
    }
    AtomicFile::replace(directory, &locator.as_relative(), &bytes)
        .map_err(crate::upload_fs_error)
}

pub(super) fn write(
    writer: &mut Writer,
    principal: &str,
    detached: Option<(&ApiError, bool)>,
) -> Result<(), ApiError> {
    let origin = origin(writer, principal)?;
    let phase = match writer.container.as_ref().unwrap().revision.as_ref() {
        Some(revision) => Phase::Frozen { revision: revision.clone() },
        None => Phase::Working,
    };
    let handle = match detached {
        Some((error, bytes_synced)) => HandleState::Detached {
            code: error.code, message: error.message.clone(), bytes_synced,
        },
        None => HandleState::Active,
    };
    let mut record = Record { format: Format::ManualWorking, origin, phase, handle };
    let name = EntryName::new(record.origin.live_name.with_extension("work.json"))
        .map_err(crate::upload_fs_error)?;
    let _lock = record_lock(&writer.directory, &name)?;
    match writer.directory.read_bounded(&name, RECORD_LIMIT) {
        Ok(bytes) => {
            let previous: Record = json_codec::from_slice(&bytes)
                .map_err(|error| RecoveryError::Codec(error).api())?;
            validate(&previous, &name, previous.origin.operation, principal)?;
            if previous.origin.context != record.origin.context
                || previous.origin.entry != record.origin.entry
                || previous.origin.base != record.origin.base
                || previous.origin.live_name != record.origin.live_name
            {
                return Err(RecoveryError::Binding.api());
            }
            if previous.origin.operation != record.origin.operation {
                let known = writer.container.as_ref().unwrap().edit.draft();
                match &previous.phase {
                    Phase::Frozen { revision } if known == Some(revision) => {}
                    Phase::Frozen { .. } | Phase::Working => {
                        return Err(RecoveryError::Binding.api());
                    }
                }
            }
            if previous.origin.operation == record.origin.operation {
                if previous.origin != record.origin {
                    return Err(RecoveryError::Binding.api());
                }
                if let Phase::Frozen { revision } = previous.phase {
                    if let Phase::Frozen { revision: proposed } = &record.phase {
                        if proposed != &revision {
                            return Err(RecoveryError::Binding.api());
                        }
                    }
                    // A live handle can lag a successful cold reconciliation.
                    // Its next Dirty must not regress durable Frozen evidence.
                    record.phase = Phase::Frozen { revision };
                }
            }
        }
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(crate::upload_fs_error(error)),
    }
    let bytes = json_codec::to_vec(&record)
        .map_err(|error| RecoveryError::Codec(error).api())?;
    if bytes.len() > RECORD_LIMIT {
        return Err(RecoveryError::Budget.api());
    }
    if detached.is_some() {
        AtomicFile::replace(&writer.directory, &name.as_relative(), &bytes)
            .map_err(crate::upload_fs_error)
    } else {
        writer.recovery_record.persist(&writer.directory, &name, bytes)
    }
}

fn read(
    directory: &PrivateDirectory,
    locator: &EntryName,
    operation: OperationId,
    principal: &str,
) -> Result<Record, ApiError> {
    let bytes = directory.read_bounded(locator, RECORD_LIMIT).map_err(crate::upload_fs_error)?;
    let record: Record = json_codec::from_slice(&bytes)
        .map_err(|error| RecoveryError::Codec(error).api())?;
    validate(&record, locator, operation, principal)?;
    Ok(record)
}

fn validate(
    record: &Record,
    locator: &EntryName,
    operation: OperationId,
    principal: &str,
) -> Result<(), ApiError> {
    let live = EntryName::new(&record.origin.live_name).map_err(crate::upload_fs_error)?;
    let expected = EntryName::new(live.as_path().with_extension("work.json"))
        .map_err(crate::upload_fs_error)?;
    if &expected != locator || record.origin.operation != operation
        || record.origin.principal.as_str() != principal
    {
        return Err(RecoveryError::Binding.api());
    }
    if let Phase::Frozen { revision } = &record.phase {
        if revision.operation != operation || revision.principal != record.origin.principal
            || revision.base != record.origin.base
            || revision.expected_generation != record.origin.expected_generation
        {
            return Err(RecoveryError::Binding.api());
        }
    }
    Ok(())
}

pub(super) fn require_frozen_reference(
    directory: &PrivateDirectory,
    live_name: &Path,
    request: &crate::PersistContainerDraft,
) -> Result<(), ApiError> {
    let locator = EntryName::new(live_name.with_extension("work.json"))
        .map_err(crate::upload_fs_error)?;
    let record = read(directory, &locator, request.revision.operation,
        request.revision.principal.as_str())?;
    if record.origin.context != request.context || record.origin.entry != request.entry {
        return Err(RecoveryError::Binding.api());
    }
    match record.phase {
        Phase::Frozen { revision } if revision == request.revision => Ok(()),
        Phase::Frozen { .. } | Phase::Working => Err(RecoveryError::OriginalEvidenceMissing.api()),
    }
}

pub(super) fn recover(
    shared: &Shared,
    identity: &Identity,
    operation: OperationId,
    locator: &str,
    limit: u64,
    resume: bool,
) -> Result<Value, ApiError> {
    let directory = PrivateDirectory::open_trusted(&shared.spool)
        .map_err(crate::upload_fs_error)?;
    let locator = EntryName::new(locator).map_err(crate::upload_fs_error)?;
    let admitted = read(&directory, &locator, operation, &identity.principal)?;
    let (_lock, mut record) = if resume {
        // Do not create lock entries for unadmitted locators. Re-read after
        // acquiring the writer-shared lock because the phase/op may have moved.
        let lock = record_lock(&directory, &locator)?;
        let current = read(&directory, &locator, operation, &identity.principal)?;
        (Some(lock), current)
    } else {
        (None, admitted)
    };
    if let Some(original) = manual_recovery::retained_revision(&directory, &record.origin)? {
        if let Phase::Frozen { revision } = &record.phase {
            if revision != &original {
                return Err(RecoveryError::Binding.api());
            }
        }
        if resume {
            persist_reference(&directory, &locator, &mut record, original)?;
        }
        return manual_recovery::recover(shared, identity, operation, limit, resume);
    }
    if let Phase::Frozen { revision } = record.phase {
        let request = crate::PersistContainerDraft {
            entry: record.origin.entry, context: record.origin.context, revision,
        };
        let receipt = shared.backend.container_draft_save_receipt(identity, request)?;
        return match receipt {
            Some(receipt) => Ok(json!({"operation_id":operation,"draft_persisted":true,
                "original_receipt":receipt,"frozen_material_missing":true})),
            None => Err(RecoveryError::Binding.api()),
        };
    }
    // No complete original Save exists yet. This read is not a receipt claim,
    // and an explicit resume freezes the retained live bytes before saving.
    shared.backend.read_container_edit(identity, record.origin.context.clone())?;
    manual_recovery::require_unrecorded(shared, identity, operation)?;
    manual_recovery::require_no_partial(&directory, operation)?;
    if !resume {
        return Ok(json!({"operation_id":operation,"unfrozen_working":true,
            "draft_persisted":false}));
    }
    let name = EntryName::new(&record.origin.live_name).map_err(crate::upload_fs_error)?;
    let live = directory.open_payload(&name, PayloadOpenMode::Existing)
        .map_err(crate::upload_fs_error)?;
    directory.verify_payload(&name, &live).map_err(crate::upload_fs_error)?;
    let revision = manual_recovery::freeze_source(
        shared, &directory, &live, &record.origin, limit,
    )?;
    persist_reference(&directory, &locator, &mut record, revision)?;
    manual_recovery::recover(shared, identity, operation, limit, true)
}
