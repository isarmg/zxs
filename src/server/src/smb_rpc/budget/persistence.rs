//! Durable ownership records: exclusive creation and atomic high-water growth.
use super::{ApiError, File, Owner, Path, Write, fs, record_path};
use std::os::unix::fs::OpenOptionsExt;

pub(super) fn create(root: &Path, source: &Path, owner: &Owner) -> Result<(), ApiError> {
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(record_path(source))
        .map_err(ApiError::io)?;
    file.write_all(
        &json_codec::to_vec(owner).expect("invariant: internal JSON serialization succeeds"),
    )
    .map_err(ApiError::io)?;
    file.sync_all().map_err(ApiError::io)?;
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(ApiError::io)
}

pub(super) fn grow(root: &Path, source: &Path, owner: &Owner) -> Result<(), ApiError> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".budget-grow-")
        .tempfile_in(root)
        .map_err(ApiError::io)?;
    temporary
        .as_file_mut()
        .write_all(
            &json_codec::to_vec(owner).expect("invariant: internal JSON serialization succeeds"),
        )
        .map_err(ApiError::io)?;
    temporary.as_file().sync_all().map_err(ApiError::io)?;
    temporary
        .persist(record_path(source))
        .map_err(|error| ApiError::io(error.error))?;
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(ApiError::io)
}
