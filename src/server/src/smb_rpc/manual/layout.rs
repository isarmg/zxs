use platform_io::fs_safety::EntryName;
use std::path::{Path, PathBuf};

pub(super) fn read_placeholder(spool: &Path) -> PathBuf {
    spool.join("empty-directory")
}

pub(super) fn source(
    operation: storage_contract::OperationId,
) -> Result<EntryName, crate::ApiError> {
    EntryName::new(format!("manual-{operation}"))
        .map_err(crate::upload_fs_error)
}

pub(super) fn record(source: &EntryName) -> Result<EntryName, crate::ApiError> {
    EntryName::new(source.as_path().with_extension("work.json"))
        .map_err(crate::upload_fs_error)
}
