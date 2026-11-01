//! Durable full-upload staging. Interrupted uploads remain attributable and queryable.
use crate::ApiError;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, PayloadOpenMode, PrivateDirectory};
use std::{
    fs,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub(crate) enum UploadState {
    Incomplete,
    ChecksumMismatch,
    Complete,
}
impl<'de> Deserialize<'de> for UploadState {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "incomplete" => Ok(Self::Incomplete),
            "checksum_mismatch" => Ok(Self::ChecksumMismatch),
            "complete" => Ok(Self::Complete),
            _ => Err(data_serialization::de::Error::custom(
                "invalid upload state",
            )),
        }
    }
}

// Only boundary DTOs implement serialization. Internal records cannot contain a missing digest.
#[derive(Clone, Debug)]
pub(crate) struct UploadRecord {
    workspace_id: String,
    item_id: String,
    principal: String,
    path: String,
    generation: u64,
    logical_size: u64,
    content_hash: storage_contract::ContentHash,
    persisted_bytes: u64,
    persisted_hash: String,
    state: UploadState,
    data_file: String,
}

// Decode owned strings at the disk boundary; borrow them for publication without cloning.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PersistedUploadRecord<S = String> {
    pub workspace_id: S,
    pub item_id: S,
    pub principal: S,
    pub path: S,
    #[serialization(with = "storage_contract::u64_string")]
    pub generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub logical_size: u64,
    pub content_hash: storage_contract::ContentHash,
    #[serialization(with = "storage_contract::u64_string")]
    pub persisted_bytes: u64,
    pub persisted_hash: S,
    pub state: UploadState,
    pub data_file: S,
}

pub(crate) struct UploadDeclaration {
    pub workspace_id: String,
    pub item_id: String,
    pub principal: String,
    pub path: String,
    pub generation: u64,
    pub logical_size: u64,
    pub content_hash: storage_contract::ContentHash,
    pub data_file: String,
}

impl UploadRecord {
    pub(crate) fn new(input: UploadDeclaration) -> Result<Self, ApiError> {
        Self::from_persisted(PersistedUploadRecord {
            workspace_id: input.workspace_id,
            item_id: input.item_id,
            principal: input.principal,
            path: input.path,
            generation: input.generation,
            logical_size: input.logical_size,
            content_hash: input.content_hash,
            data_file: input.data_file,
            persisted_bytes: 0,
            persisted_hash: blake3::hash(b"").to_hex().to_string(),
            state: UploadState::Incomplete,
        })
    }

    fn from_persisted(record: PersistedUploadRecord) -> Result<Self, ApiError> {
        if record.persisted_bytes > record.logical_size
            || record.data_file.contains('/')
            || record.data_file.contains('\\')
            || record.data_file.starts_with('.')
            || record.data_file.is_empty()
            || record.generation == 0
            || record.persisted_hash.len() != 64
            || !record
                .persisted_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::UploadStateInvalid,
                "invalid upload state",
            ));
        }
        Ok(Self {
            workspace_id: record.workspace_id,
            item_id: record.item_id,
            principal: record.principal,
            path: record.path,
            generation: record.generation,
            logical_size: record.logical_size,
            content_hash: record.content_hash,
            persisted_bytes: record.persisted_bytes,
            persisted_hash: record.persisted_hash,
            state: record.state,
            data_file: record.data_file,
        })
    }

    pub(crate) fn persisted(&self) -> PersistedUploadRecord<&str> {
        let Self {
            workspace_id,
            item_id,
            principal,
            path,
            generation,
            logical_size,
            content_hash,
            persisted_bytes,
            persisted_hash,
            state,
            data_file,
        } = self;
        PersistedUploadRecord {
            workspace_id,
            item_id,
            principal,
            path,
            generation: *generation,
            logical_size: *logical_size,
            content_hash: *content_hash,
            persisted_bytes: *persisted_bytes,
            persisted_hash,
            state: *state,
            data_file,
        }
    }

    pub(crate) fn principal(&self) -> &str {
        &self.principal
    }
    pub(crate) fn path(&self) -> &str {
        &self.path
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn logical_size(&self) -> u64 {
        self.logical_size
    }
    pub(crate) fn content_hash(&self) -> storage_contract::ContentHash {
        self.content_hash
    }
    pub(crate) fn persisted_bytes(&self) -> u64 {
        self.persisted_bytes
    }
    pub(crate) fn persisted_hash(&self) -> &str {
        &self.persisted_hash
    }
    pub(crate) fn state(&self) -> UploadState {
        self.state
    }
    pub(crate) fn data_file(&self) -> &str {
        &self.data_file
    }

    pub(crate) fn checkpoint(&mut self, length: u64, hash: blake3::Hash) {
        assert!(
            length <= self.logical_size,
            "invariant: upload progress is bounded by declared length"
        );
        self.persisted_bytes = length;
        self.persisted_hash = hash.to_hex().to_string();
    }

    pub(crate) fn set_state(&mut self, state: UploadState) {
        self.state = state;
    }
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
            let record = storage_contract::decode_strict::<PersistedUploadRecord>(&bytes).map_err(
                |source| {
                    ApiError::new(
                        ::http::StatusCode::INTERNAL_SERVER_ERROR,
                        storage_contract::ErrorCode::UploadStateInvalid,
                        "invalid upload state",
                    )
                    .with_source(source)
                },
            )?;
            if record.workspace_id != workspace || record.item_id != item {
                return Err(ApiError::new(
                    ::http::StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::UploadStateInvalid,
                    "invalid upload state",
                ));
            }
            Ok(Some(UploadRecord::from_persisted(record)?))
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
        mode: PayloadOpenMode,
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
            .open_payload(&name, mode)
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
    pub fn source(
        &self,
        size: u64,
        hash: storage_contract::ContentHash,
    ) -> Result<crate::UploadSource, ApiError> {
        let writer = crate::lock(&self.writer);
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
        let mut writer = crate::lock(&self.writer);
        self.reservation.check_headroom(
            writer.metadata().map_err(ApiError::io)?.len(),
            bytes.len() as u64,
        )?;
        writer.write_all(bytes).map_err(ApiError::io)
    }
    pub fn sync(&self) -> Result<(), ApiError> {
        crate::lock(&self.writer).sync_all().map_err(ApiError::io)
    }
    pub fn block_admission(&self) {
        self.reservation.block_admission();
    }
}
pub(crate) fn persist_staged(record: &UploadRecord, staged: &StagedFile) -> Result<(), ApiError> {
    let result = (|| {
        staged
            .directory
            .verify_identity()
            .map_err(crate::upload_fs_error)?;
        let name = format!("{}.json", key(&record.workspace_id, &record.item_id));
        persist_held(&staged.directory, &name, record)
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
    let name = format!("{}.json", key(&record.workspace_id, &record.item_id));
    persist_at(root, &name, record)
}
fn persist_at(root: &Path, name: &str, record: &UploadRecord) -> Result<(), ApiError> {
    let directory = platform_io::fs_safety::PrivateDirectory::open_trusted(root)
        .map_err(crate::upload_fs_error)?;
    persist_held(&directory, name, record)
}
fn persist_held(
    directory: &platform_io::fs_safety::PrivateDirectory,
    name: &str,
    record: &UploadRecord,
) -> Result<(), ApiError> {
    use platform_io::fs_safety::{AtomicFile, RelativePath};
    let bytes = json_codec::to_vec(&record.persisted())
        .expect("invariant: internal JSON serialization succeeds");
    if bytes.len() > 16384 {
        return Err(ApiError::invalid("upload metadata exceeds limit"));
    }
    AtomicFile::replace(
        directory,
        &RelativePath::new(name).map_err(crate::upload_fs_error)?,
        &bytes,
    )
    // replace syncs the temporary file and this same held parent after rename.
    .map_err(crate::upload_fs_error)
}

// HTTP projection is separate from the durable record. In particular, neither
// owner identity nor filesystem paths belong in the public status response.
#[derive(Serialize)]
#[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
enum UploadStatusState {
    Receiving,
    ChecksumMismatch,
    Complete,
}
impl From<UploadState> for UploadStatusState {
    fn from(state: UploadState) -> Self {
        match state {
            UploadState::Incomplete => Self::Receiving,
            UploadState::ChecksumMismatch => Self::ChecksumMismatch,
            UploadState::Complete => Self::Complete,
        }
    }
}

#[derive(Serialize)]
struct UploadStatus<'a> {
    workspace_id: &'a str,
    item_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    upload_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    logical_size: u64,
    content_hash: storage_contract::ContentHash,
    state: UploadStatusState,
    durable_ranges: Vec<ReceivedRange<'a>>,
    sealed_manifest_hash: Option<&'a str>,
}
#[derive(Serialize)]
struct ReceivedRange<'a> {
    #[serialization(with = "storage_contract::u64_string")]
    offset: u64,
    #[serialization(with = "storage_contract::u64_string")]
    length: u64,
    content_hash: &'a str,
}

pub(crate) fn status(record: &UploadRecord) -> json_codec::Value {
    let UploadRecord {
        workspace_id,
        item_id,
        generation,
        logical_size,
        content_hash,
        persisted_bytes,
        persisted_hash: _,
        state: _,
        principal: _,
        path: _,
        data_file: _,
    } = record;
    let status = UploadStatus {
        workspace_id,
        item_id,
        upload_generation: *generation,
        logical_size: *logical_size,
        content_hash: *content_hash,
        state: record.state().into(),
        durable_ranges: if *persisted_bytes > 0 {
            vec![ReceivedRange {
                offset: 0,
                length: *persisted_bytes,
                content_hash: record.persisted_hash(),
            }]
        } else {
            vec![]
        },
        sealed_manifest_hash: None,
    };
    json_codec::to_value(status).expect("invariant: upload status serialization succeeds")
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
    persist_at(root, &name, record)?;
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
    Ok(())
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
            .open_payload(&name, PayloadOpenMode::Existing)
            .map_err(crate::upload_fs_error)?;
        let length = file.metadata().map_err(ApiError::io)?.len();
        if length < record.persisted_bytes() || length > record.logical_size {
            return Err(ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
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
        match self.claims.lock() {
            Ok(mut claims) => {
                claims.remove(&self.key);
            }
            Err(_) => {
                // Preserve ownership and poison: no subsequent admission may trust this index.
                tracing::error!("claims cleanup blocked by poisoned registry");
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/uploads/tests.rs"]
mod tests;
