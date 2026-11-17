use platform_io::fs_safety::EntryName;
use storage_contract::OperationId;

pub(super) fn source(operation: OperationId) -> Result<EntryName, crate::ApiError> {
    EntryName::new(format!("worktree-{operation}")).map_err(crate::upload_fs_error)
}

pub(super) fn record(source: &EntryName) -> Result<EntryName, crate::ApiError> {
    EntryName::new(source.as_path().with_extension("work.json")).map_err(crate::upload_fs_error)
}
