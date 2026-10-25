//! Durable full-upload staging. Interrupted uploads remain attributable and queryable.
use crate::ApiError;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use std::{
    fs,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct UploadRecord {
    pub workspace_id: String,
    pub item_id: String,
    pub principal: String,
    pub path: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub logical_size: u64,
    pub content_hash: Option<String>,
    #[serialization(with = "storage_contract::u64_string")]
    pub persisted_bytes: u64,
    pub persisted_hash: String,
    pub state: String,
    pub data_file: String,
}
pub(crate) fn key(workspace: &str, item: &str) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(workspace.as_bytes());
    hash.update(&[0]);
    hash.update(item.as_bytes());
    hash.finalize().to_hex().to_string()
}
pub(crate) fn load(
    root: &Path,
    workspace: &str,
    item: &str,
) -> Result<Option<UploadRecord>, ApiError> {
    let directory = PrivateDirectory::open_trusted(root).map_err(crate::upload_fs_error)?;
    let name =
        EntryName::new(format!("{}.json", key(workspace, item))).map_err(crate::upload_fs_error)?;
    match directory.read_bounded(&name, 16384) {
        Ok(bytes) => {
            let record = storage_contract::decode_strict::<UploadRecord>(&bytes).map_err(|_| {
                ApiError::new(
                    500,
                    storage_contract::ErrorCode::UploadStateInvalid,
                    "invalid upload state",
                )
            })?;
            if record.workspace_id != workspace
                || record.item_id != item
                || record.persisted_bytes > record.logical_size
                || record.data_file.contains('/')
                || record.data_file.contains('\\')
                || record.data_file.starts_with('.')
                || record.data_file.is_empty()
                || record.generation == 0
                || record.content_hash.as_ref().is_none_or(|hash| {
                    hash.len() != 64
                        || !hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
                || record.persisted_hash.len() != 64
                || !record
                    .persisted_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(ApiError::new(
                    500,
                    storage_contract::ErrorCode::UploadStateInvalid,
                    "invalid upload state",
                ));
            }
            Ok(Some(record))
        }
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(crate::upload_fs_error(error)),
    }
}
/// Every filesystem task shares this owner. Cancellation cannot settle its
/// reservation while a blocking write or metadata publication is still running.
/// Retained files are never deleted merely because an HTTP request ended.
pub(crate) struct StagedFile {
    writer: std::sync::Mutex<fs::File>,
    directory: platform_io::fs_safety::PrivateDirectory,
    name: platform_io::fs_safety::EntryName,
    // Dropped after the writer; settlement inventories the actual file length.
    reservation: crate::spool_budget::Reservation,
}
impl StagedFile {
    pub fn open_budgeted(
        path: PathBuf,
        retry: bool,
        reservation: crate::spool_budget::Reservation,
    ) -> Result<std::sync::Arc<Self>, ApiError> {
        reservation.persist_owner()?;
        use platform_io::fs_safety::{EntryName, PrivateDirectory};
        let directory = PrivateDirectory::open_trusted(
            path.parent()
                .ok_or_else(|| ApiError::invalid("upload parent missing"))?,
        )
        .map_err(crate::upload_fs_error)?;
        let name = EntryName::new(
            path.file_name()
                .ok_or_else(|| ApiError::invalid("upload filename missing"))?,
        )
        .map_err(crate::upload_fs_error)?;
        let mut writer = directory
            .open_payload(&name, !retry)
            .map_err(crate::upload_fs_error)?;
        writer
            .seek(std::io::SeekFrom::End(0))
            .map_err(ApiError::io)?;
        Ok(std::sync::Arc::new(Self {
            writer: std::sync::Mutex::new(writer),
            directory,
            name,
            reservation,
        }))
    }
    pub fn source(&self, size: u64, hash: String) -> Result<crate::UploadSource, ApiError> {
        let writer = crate::lock(&self.writer)?;
        self.directory
            .verify_payload(&self.name, &writer)
            .map_err(crate::upload_fs_error)?;
        Ok(crate::UploadSource {
            file: writer.try_clone().map_err(ApiError::io)?,
            size,
            hash,
        })
    }
    pub fn write(&self, bytes: &[u8]) -> Result<(), ApiError> {
        let mut writer = crate::lock(&self.writer)?;
        self.reservation.check_headroom(
            writer.metadata().map_err(ApiError::io)?.len(),
            bytes.len() as u64,
        )?;
        writer.write_all(bytes).map_err(ApiError::io)
    }
    pub fn sync(&self) -> Result<(), ApiError> {
        crate::lock(&self.writer)?.sync_all().map_err(ApiError::io)
    }
    pub fn block_admission(&self) {
        self.reservation.block_admission();
    }
}
pub(crate) fn persist_staged(
    root: &Path,
    record: &UploadRecord,
    staged: &StagedFile,
) -> Result<(), ApiError> {
    persist_staged_with_sync(root, record, staged, || {
        staged.directory.sync().map_err(crate::upload_fs_error)
    })
}
fn sync_directory(root: &Path) -> Result<(), ApiError> {
    fs::File::open(root)
        .and_then(|file| file.sync_all())
        .map_err(ApiError::io)
}
fn persist_staged_with_sync(
    _root: &Path,
    record: &UploadRecord,
    staged: &StagedFile,
    sync: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let result = (|| {
        staged
            .directory
            .verify_identity()
            .map_err(crate::upload_fs_error)?;
        let name = format!("{}.json", key(&record.workspace_id, &record.item_id));
        persist_held(&staged.directory, &name, record, || {}, sync)
    })();
    if result.is_err() {
        // A failed cleanup or directory sync can leave extra/uncertain metadata.
        // No fresh upload may assume that this evidence consumes no capacity.
        staged.block_admission();
    }
    result
}
#[cfg(test)]
pub(crate) fn persist(root: &Path, record: &UploadRecord) -> Result<(), ApiError> {
    persist_inner(root, record, || {}, || sync_directory(root))
}
#[cfg(test)]
fn persist_inner(
    root: &Path,
    record: &UploadRecord,
    published: impl FnOnce(),
    sync: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let name = format!("{}.json", key(&record.workspace_id, &record.item_id));
    persist_at(root, &name, record, published, sync)
}
fn persist_at(
    root: &Path,
    name: &str,
    record: &UploadRecord,
    published: impl FnOnce(),
    sync: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let directory = platform_io::fs_safety::PrivateDirectory::open_trusted(root)
        .map_err(crate::upload_fs_error)?;
    persist_held(&directory, name, record, published, sync)
}
fn persist_held(
    directory: &platform_io::fs_safety::PrivateDirectory,
    name: &str,
    record: &UploadRecord,
    published: impl FnOnce(),
    sync: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    use platform_io::fs_safety::{AtomicFile, RelativePath};
    let bytes = json_codec::to_vec(record).map_err(ApiError::invalid)?;
    if bytes.len() > 16384 {
        return Err(ApiError::invalid("upload metadata exceeds limit"));
    }
    AtomicFile::replace(
        directory,
        &RelativePath::new(name).map_err(crate::upload_fs_error)?,
        &bytes,
    )
    .map_err(crate::upload_fs_error)?;
    published();
    sync()
}

pub(crate) fn status(record: &UploadRecord) -> json_codec::Value {
    json_codec::json!({"workspace_id":record.workspace_id,"item_id":record.item_id,"upload_generation":record.generation.to_string(),"logical_size":record.logical_size.to_string(),"content_hash":record.content_hash,"state":if record.state=="complete"{"COMPLETE"}else{"RECEIVING"},"durable_ranges":if record.persisted_bytes>0{vec![json_codec::json!({"offset":"0","length":record.persisted_bytes.to_string(),"content_hash":record.persisted_hash})]}else{vec![]},"sealed_manifest_hash":null})
}
#[cfg(test)]
pub(crate) struct RetainGenerationHook {
    pub root: PathBuf,
    pub after_publish: std::sync::Arc<dyn Fn() + Send + Sync>,
}
#[cfg(test)]
pub(crate) static RETAIN_GENERATION_HOOK: std::sync::Mutex<Option<RetainGenerationHook>> =
    std::sync::Mutex::new(None);

pub(crate) fn retain_generation(root: &Path, record: &UploadRecord) -> Result<(), ApiError> {
    let name = format!(
        "retained-{}-{}-{}.json",
        key(&record.workspace_id, &record.item_id),
        record.generation,
        blake3::hash(record.data_file.as_bytes()).to_hex()
    );
    persist_at(
        root,
        &name,
        record,
        || {
            #[cfg(test)]
            {
                let hook = RETAIN_GENERATION_HOOK
                    .lock()
                    .unwrap()
                    .as_ref()
                    .filter(|hook| hook.root == root)
                    .map(|hook| hook.after_publish.clone());
                if let Some(hook) = hook {
                    hook();
                }
            }
        },
        || sync_directory(root),
    )
}
pub(crate) struct ExistingPrefix {
    file: fs::File,
    remaining: u64,
}
impl ExistingPrefix {
    pub fn open(root: &Path, record: &UploadRecord) -> Result<Option<Self>, ApiError> {
        let directory = PrivateDirectory::open_trusted(root).map_err(crate::upload_fs_error)?;
        let name = EntryName::new(&record.data_file).map_err(crate::upload_fs_error)?;
        let file = directory
            .open_payload(&name, false)
            .map_err(crate::upload_fs_error)?;
        let length = file.metadata().map_err(ApiError::io)?.len();
        if length < record.persisted_bytes || length > record.logical_size {
            return Err(ApiError::new(
                500,
                storage_contract::ErrorCode::UploadStateInvalid,
                "persisted upload data is missing",
            ));
        }
        Ok((length > 0).then_some(Self {
            file,
            remaining: length,
        }))
    }
    pub fn remaining(&self) -> u64 {
        self.remaining
    }
    pub fn check(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        let length = bytes
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let mut expected = vec![0u8; length];
        self.file.read_exact(&mut expected).map_err(ApiError::io)?;
        if expected != bytes[..length] {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::UploadRangeConflict,
                "same upload generation contains different bytes; retain this generation and explicitly start a newer one",
            ));
        }
        self.remaining -= length as u64;
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.remaining == 0
    }
}
pub(crate) struct Claim {
    pub claims: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    pub key: String,
}
impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut claims) = self.claims.lock() {
            claims.remove(&self.key);
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/uploads/tests.rs"]
mod tests;
