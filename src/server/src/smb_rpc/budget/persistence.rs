//! Durable ownership records anchored to the inventory owner's directory.
use super::{ApiError, Owner};
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory};

pub(super) fn create(
    directory: &PrivateDirectory,
    name: &EntryName,
    owner: &Owner,
) -> Result<(), ApiError> {
    directory
        .verify_identity()
        .map_err(crate::upload_fs_error)?;
    let bytes = json_codec::to_vec(owner).expect("invariant: internal JSON serialization succeeds");
    AtomicFile::create(directory, name, &bytes).map_err(crate::upload_fs_error)
}

pub(super) fn grow(
    directory: &PrivateDirectory,
    name: &EntryName,
    owner: &Owner,
) -> Result<(), ApiError> {
    directory
        .verify_identity()
        .map_err(crate::upload_fs_error)?;
    let bytes = json_codec::to_vec(owner).expect("invariant: internal JSON serialization succeeds");
    AtomicFile::replace(directory, &name.as_relative(), &bytes).map_err(crate::upload_fs_error)
}

/// Unlink only this completed operation's files and durably settle the directory.
pub(super) fn cleanup(directory: &PrivateDirectory, names: &[EntryName]) -> Result<(), ApiError> {
    directory
        .remove_files(names)
        .map_err(crate::upload_fs_error)
}
