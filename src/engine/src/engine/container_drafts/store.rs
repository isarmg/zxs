//! Immutable draft preparation. Only a verified SYSTEM publication can select
//! a revision for shared reads; local intent files never establish current state.
use crate::disk::{DiskFile, DiskHandle, DiskRoot, UploadFinish, UploadIo};
use crate::persist::layout::container_drafts as layout;
use data_serialization::{Deserialize, Serialize};
use std::{fs::File, io::Seek, os::unix::fs::FileExt, path::PathBuf};
use storage_contract::{ChecksumManifest, FileId, Generation};
use storage_contract::container_edit::DraftRevision;
use storage_contract::working_tree::{DiskRegistrationIdentity, LocalEntityIdentity};

const MAX_INTENT: usize = 64 * 1024;
type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, error_definition::Error)]
pub enum StoreError {
    #[error("draft storage I/O failed")]
    Engine(#[source] Box<crate::Error>),
    #[error("draft metadata encoding failed")]
    Codec(#[source] storage_contract::Error),
    #[error("draft body does not match its publication reference")]
    Payload(#[source] platform_io::payload::Error),
    #[error("draft operation conflicts with retained original evidence")]
    Conflict,
    #[error("draft preparation is incomplete or contains unknown material; bytes preserved")]
    Incomplete,
    #[error("draft preparation exceeds its retained byte or operation budget")]
    Budget,
}
fn engine(error: crate::Error) -> StoreError {
    StoreError::Engine(Box::new(error))
}

#[derive(Clone, Copy)]
pub(crate) struct DraftLimits {
    pub(crate) max_file_bytes: u64,
    pub(crate) checksum_chunk_bytes: u32,
    pub(crate) max_entries: usize,
    pub(crate) max_retained_bytes: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "container-draft-intent-v1")]
    Draft,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    format: Format,
    file: FileId,
    expected: Generation,
    revision: DraftRevision,
    disk: DiskRegistrationIdentity,
    root: LocalEntityIdentity,
}
impl Intent {
    fn new(disk: &DiskHandle, file: FileId, expected: Generation, revision: &DraftRevision) -> Self {
        Self {
            format: Format::Draft, file, expected, revision: revision.clone(),
            disk: DiskRegistrationIdentity {
                pool_id: disk.seal.pool_id.clone(),
                disk_id: disk.seal.disk_id.clone(),
                slot_id: disk.seal.slot_id.clone(),
                binding_generation: disk.seal.binding_generation,
            },
            root: LocalEntityIdentity::new(disk.seal.device, disk.seal.inode),
        }
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = storage_contract::canonical_json(self).map_err(StoreError::Codec)?;
        if bytes.len() > MAX_INTENT {
            return Err(StoreError::Budget);
        }
        Ok(bytes)
    }
}

/// A verified leased body, not a capability to publish or choose this revision.
/// The caller must bind it to its original expected state at publication time.
pub(crate) struct PreparedDraft {
    file: FileId,
    expected: Generation,
    revision: DraftRevision,
    body: DiskFile,
    checksums: ChecksumManifest,
}
impl PreparedDraft {
    pub(crate) fn file(&self) -> FileId {
        self.file
    }
    pub(crate) fn expected(&self) -> Generation {
        self.expected
    }
    pub(crate) fn revision(&self) -> &DraftRevision {
        &self.revision
    }
    pub(crate) fn into_body(self) -> DiskFile {
        self.body
    }
    pub(crate) fn into_read_parts(self) -> (DiskFile, ChecksumManifest) {
        (self.body, self.checksums)
    }
}
fn directory(disk: &DiskHandle, file: FileId, revision: &DraftRevision) -> PathBuf {
    disk.root.path().join(layout::ROOT.as_path()).join(file.to_string())
        .join(revision.operation.to_string())
}
fn require_limits(revision: &DraftRevision, limits: DraftLimits) -> Result<()> {
    if revision.size > limits.max_file_bytes || revision.size > limits.max_retained_bytes
        || limits.max_entries == 0
        || !storage_contract::SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&limits.checksum_chunk_bytes)
    {
        return Err(StoreError::Budget);
    }
    Ok(())
}
fn inventory(root: &DiskRoot, path: &std::path::Path) -> Result<()> {
    for entry in root.entries(path).map_err(engine)? {
        let entry = entry.map_err(|error| engine(error.into()))?;
        if entry != *layout::INTENT && entry != *layout::BODY {
            return Err(StoreError::Incomplete);
        }
    }
    Ok(())
}
fn require_intent(root: &DiskRoot, path: &std::path::Path, expected: &Intent) -> Result<()> {
    inventory(root, path)?;
    let intent_path = path.join(layout::INTENT.as_path());
    if root.optional_metadata(&intent_path).map_err(engine)?.is_none() {
        return Err(StoreError::Incomplete);
    }
    let bytes = root.read_bytes(&intent_path, MAX_INTENT).map_err(engine)?;
    let original: Intent = storage_contract::decode_strict(&bytes).map_err(StoreError::Codec)?;
    if original != *expected || original.bytes()? != bytes {
        return Err(StoreError::Conflict);
    }
    Ok(())
}
// One original store-root descriptor and lock cover all files, including
// unpublished preparations. Unknown partial material blocks new allocation;
// it is neither skipped for accounting nor deleted to make room.
fn retained_budget(
    root: &DiskRoot, intent: &Intent, intent_bytes: usize, limits: DraftLimits,
) -> Result<()> {
    let mut files = 0usize;
    let mut operations = 0usize;
    let mut bytes = 0u64;
    let mut file_present = false;
    let mut present = false;
    let mut body_present = false;
    for entry in root.entries(root.path()).map_err(engine)? {
        let entry = entry.map_err(|error| engine(error.into()))?;
        files = files.checked_add(1).ok_or(StoreError::Budget)?;
        if files > limits.max_entries {
            return Err(StoreError::Budget);
        }
        let file = entry.as_os_str().to_str().ok_or(StoreError::Incomplete)?
            .parse::<FileId>().map_err(|_| StoreError::Incomplete)?;
        file_present |= file == intent.file;
        let file_root = root.private_child(&root.path().join(entry.as_path())).map_err(engine)?;
        for entry in file_root.entries(file_root.path()).map_err(engine)? {
            let entry = entry.map_err(|error| engine(error.into()))?;
            operations = operations.checked_add(1).ok_or(StoreError::Budget)?;
            if operations > limits.max_entries {
                return Err(StoreError::Budget);
            }
            let operation = entry.as_os_str().to_str().ok_or(StoreError::Incomplete)?
                .parse::<storage_contract::OperationId>().map_err(|_| StoreError::Incomplete)?;
            let path = file_root.path().join(entry.as_path());
            let operation_root = file_root.private_child(&path).map_err(engine)?;
            inventory(&operation_root, &path)?;
            let original = operation_root.read_bytes(
                &path.join(layout::INTENT.as_path()), MAX_INTENT,
            ).map_err(engine)?;
            let decoded: Intent = storage_contract::decode_strict(&original)
                .map_err(StoreError::Codec)?;
            if decoded.file != file || decoded.revision.operation != operation
                || decoded.bytes()? != original || decoded.disk != intent.disk
                || decoded.root != intent.root
            {
                return Err(StoreError::Conflict);
            }
            bytes = bytes.checked_add(original.len() as u64).ok_or(StoreError::Budget)?;
            let selected = file == intent.file && operation == intent.revision.operation;
            present |= selected;
            if let Some(body) = operation_root.optional_metadata(&path.join(layout::BODY.as_path()))
                .map_err(engine)?
            {
                if !body.is_file() || body.file_type().is_symlink() {
                    return Err(StoreError::Incomplete);
                }
                bytes = bytes.checked_add(body.len()).ok_or(StoreError::Budget)?;
                body_present |= selected;
            }
            if bytes > limits.max_retained_bytes {
                return Err(StoreError::Budget);
            }
        }
    }
    if !file_present && files >= limits.max_entries {
        return Err(StoreError::Budget);
    }
    if !present {
        operations = operations.checked_add(1).ok_or(StoreError::Budget)?;
        bytes = bytes.checked_add(intent_bytes as u64).ok_or(StoreError::Budget)?;
    }
    if !body_present {
        bytes = bytes.checked_add(intent.revision.size).ok_or(StoreError::Budget)?;
    }
    if operations > limits.max_entries || bytes > limits.max_retained_bytes {
        return Err(StoreError::Budget);
    }
    Ok(())
}

/// Persist the exact original intent before body I/O. A directory with no
/// original intent or unknown partial files is preserved and cannot be adopted.
/// Retry of intent-only evidence may supply the same body; completed bytes are
/// verified, never overwritten. Prior revisions are never removed by this store.
pub(crate) fn prepare(
    disk: &DiskHandle, file: FileId, expected: Generation, revision: &DraftRevision,
    source: File, limits: DraftLimits,
) -> Result<PreparedDraft> {
    require_limits(revision, limits)?;
    if revision.expected_generation != expected {
        return Err(StoreError::Conflict);
    }
    let intent = Intent::new(disk, file, expected, revision);
    let bytes = intent.bytes()?;
    let path = directory(disk, file, revision);
    let store_path = disk.root.path().join(layout::ROOT.as_path());
    disk.root.private_dir(&store_path).map_err(engine)?;
    let root = disk.root.private_child(&store_path).map_err(engine)?;
    let directory_handle = root.directory(root.path()).map_err(engine)?;
    let _lock = platform_io::fs_safety::linux::AdvisoryLock::directory(&directory_handle)
        .map_err(|error| engine(error.into()))?;
    retained_budget(&root, &intent, bytes.len(), limits)?;
    // Admit the typed file parent before probing the operation leaf.
    // Missing ancestors are I/O failures, not evidence of an absent operation.
    let parent = path.parent().ok_or(StoreError::Incomplete)?;
    root.private_dir(parent).map_err(engine)?;
    if root.optional_metadata(&path).map_err(engine)?.is_some() {
        require_intent(&root, &path, &intent)?;
    } else {
        root.create_private_directory_synced(&path).map_err(engine)?;
        root.write_atomic(&path.join(layout::INTENT.as_path()), &bytes, false).map_err(engine)?;
    }
    let io = UploadIo::new(&root, &path, source).map_err(engine)?;
    let source = io.fingerprint_source(limits.max_file_bytes).map_err(engine)?;
    if source != (revision.size, revision.content_hash) {
        return Err(StoreError::Conflict);
    }
    if io.target_exists(&layout::BODY).map_err(engine)? {
        io.verify_retained(&layout::BODY, revision.size, &revision.content_hash,
            limits.max_file_bytes).map_err(engine)?;
    } else {
        let copied = io.copy(layout::SCRATCH.clone(), limits.checksum_chunk_bytes,
            revision.size).map_err(engine)?;
        if copied != (revision.size, revision.content_hash) {
            return Err(StoreError::Conflict);
        }
        let identity = root.directory_identity(&path).map_err(engine)?;
        let maximum = limits.max_file_bytes.checked_add(MAX_INTENT as u64)
            .ok_or(StoreError::Budget)?;
        io.finish(&layout::BODY, identity, UploadFinish::Publish {
            max_workspace_bytes: maximum, max_entries: 3,
        }).map_err(engine)?;
    }
    drop(io);
    reopen_at(&root, disk, file, expected, revision, limits.checksum_chunk_bytes)
}

/// The revision and original expected generation must come from an admitted
/// publication/receipt, never from scanning local intents for the newest body.
pub(crate) fn reopen(
    disk: &DiskHandle, file: FileId, expected: Generation, revision: &DraftRevision,
    limits: DraftLimits,
) -> Result<PreparedDraft> {
    require_limits(revision, limits)?;
    if revision.expected_generation != expected {
        return Err(StoreError::Conflict);
    }
    let root = disk.root.private_child(&disk.root.path().join(layout::ROOT.as_path()))
        .map_err(engine)?;
    reopen_at(&root, disk, file, expected, revision, limits.checksum_chunk_bytes)
}
fn reopen_at(
    root: &DiskRoot, disk: &DiskHandle, file: FileId, expected: Generation,
    revision: &DraftRevision, chunk_bytes: u32,
) -> Result<PreparedDraft> {
    let path = directory(disk, file, revision);
    require_intent(root, &path, &Intent::new(disk, file, expected, revision))?;
    let mut body = root.open_leased_regular(&path.join(layout::BODY.as_path()))
        .map_err(engine)?;
    body.verify_full(platform_io::payload::Expectation {
        bytes: revision.size, blake3: Some(revision.content_hash.digest()),
    }).map_err(StoreError::Payload)?;
    let checksums = read_checksums(&body, revision, chunk_bytes)?;
    body.rewind().map_err(|error| engine(error.into()))?;
    Ok(PreparedDraft { file, expected, revision: revision.clone(), body, checksums })
}

// A manifest is derived from this exact revocable descriptor, not a sidecar or
// a fabricated version. Its full hash is checked against the selected revision.
fn read_checksums(
    body: &DiskFile, revision: &DraftRevision, chunk_bytes: u32,
) -> Result<ChecksumManifest> {
    let count = revision.size.div_ceil(u64::from(chunk_bytes));
    let encoded = count.checked_mul(32)
        .and_then(|bytes| bytes.checked_add(storage_contract::CHECKSUM_HEADER_BYTES as u64))
        .ok_or(StoreError::Budget)?;
    if encoded > crate::disk::MAX_METADATA as u64 {
        return Err(StoreError::Budget);
    }
    let before = crate::file_stamp::FileStamp::from_metadata(
        &body.metadata().map_err(|error| engine(error.into()))?,
    );
    let mut chunks = Vec::new();
    let mut buffer = vec![0; chunk_bytes as usize];
    let mut offset = 0u64;
    let mut full = blake3::Hasher::new();
    while offset < revision.size {
        let length = (revision.size - offset).min(u64::from(chunk_bytes)) as usize;
        body.check_running().map_err(engine)?;
        body.read_exact_at(&mut buffer[..length], offset)
            .map_err(|error| engine(error.into()))?;
        full.update(&buffer[..length]);
        chunks.push(*blake3::hash(&buffer[..length]).as_bytes());
        offset += length as u64;
    }
    let after = crate::file_stamp::FileStamp::from_metadata(
        &body.metadata().map_err(|error| engine(error.into()))?,
    );
    if before != after || before.size != revision.size
        || full.finalize() != revision.content_hash.digest()
    {
        return Err(StoreError::Conflict);
    }
    body.check_running().map_err(engine)?;
    Ok(ChecksumManifest { chunk_bytes, logical_size: revision.size, chunks })
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/container_draft_store_tests.rs"]
mod tests;
