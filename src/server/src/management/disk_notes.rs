//! System-disk notes keyed by the current registered disk ID, not a hardware path.
//! The caller serializes reads and updates and admits the current registration.
use crate::ApiError;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AtomicFile, EntryName, Error, PrivateDirectory};
use std::path::Path;

const DIRECTORY: &str = "disk-notes";

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: String,
    disk_id: String,
    note: String,
}

fn file_name(disk_id: &str) -> EntryName {
    // IDs come only from a validated engine configuration, never the request.
    EntryName::new(format!("{disk_id}.json"))
        .expect("invariant: registered disk ID is a single filename component")
}

fn storage_error(source: Error) -> ApiError {
    ApiError::new(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::IoError,
        "disk note storage operation failed",
    )
    .with_source(source)
}

fn read_record(directory: &PrivateDirectory, disk_id: &str) -> Result<String, ApiError> {
    let bytes = match directory.read_bounded(&file_name(disk_id), crate::JSON_BODY_LIMIT) {
        Ok(bytes) => bytes,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(String::new());
        }
        Err(error) => return Err(storage_error(error)),
    };
    let value = storage_contract::strict_json(&bytes).map_err(|source| {
        ApiError::new(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::ObjectCorrupt,
            "disk note record cannot be decoded",
        )
        .with_source(source)
    })?;
    if !value.is_object() {
        return Err(corrupt());
    }
    let record: Record =
        json_codec::from_value(value).map_err(|source| corrupt().with_source(source))?;
    if record.format != "disk-note" || record.disk_id != disk_id {
        return Err(corrupt());
    }
    Ok(record.note)
}

fn corrupt() -> ApiError {
    ApiError::new(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::ObjectCorrupt,
        "disk note record has an unsupported format or different registered disk identity",
    )
}

pub(super) fn read(root: &Path, disk_id: &str) -> Result<String, ApiError> {
    let parent = PrivateDirectory::open_trusted(root).map_err(storage_error)?;
    let directory = match PrivateDirectory::open_trusted(root.join(DIRECTORY)) {
        Ok(directory) => directory,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            parent.verify_identity().map_err(storage_error)?;
            return Ok(String::new());
        }
        Err(error) => return Err(storage_error(error)),
    };
    let note = read_record(&directory, disk_id)?;
    parent.verify_identity().map_err(storage_error)?;
    directory.verify_identity().map_err(storage_error)?;
    Ok(note)
}

pub(super) fn update(
    root: &Path,
    disk_id: &str,
    expected: &str,
    note: String,
) -> Result<String, ApiError> {
    let record = Record {
        format: "disk-note".into(),
        disk_id: disk_id.into(),
        note,
    };
    let bytes = json_codec::to_vec(&record)
        .expect("invariant: disk note strings have a JSON representation");
    // Reuse the existing HTTP input budget, including for direct Backend callers.
    // The persisted envelope is smaller than its corresponding update request.
    if bytes.len() > crate::JSON_BODY_LIMIT {
        return Err(ApiError::new(
            http::StatusCode::PAYLOAD_TOO_LARGE,
            storage_contract::ErrorCode::PayloadTooLarge,
            "disk note exceeds the request body limit",
        ));
    }
    let parent = PrivateDirectory::open_trusted(root).map_err(storage_error)?;
    let directory = parent
        .create_child(&EntryName::new(DIRECTORY).expect("invariant: static directory name"))
        .map_err(storage_error)?;
    let current = read_record(&directory, disk_id)?;
    if current != expected && current != record.note {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::GenerationMismatch,
            "disk note changed; reload before replacing it",
        ));
    }
    parent.verify_identity().map_err(storage_error)?;
    directory.verify_identity().map_err(storage_error)?;
    // Even a same-value retry crosses the file and directory durability barriers:
    // a previous publication may have succeeded without confirming its fsync.
    AtomicFile::replace(&directory, &file_name(disk_id).as_relative(), &bytes)
        .map_err(storage_error)?;
    Ok(record.note)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/disk_notes.rs"]
mod tests;
