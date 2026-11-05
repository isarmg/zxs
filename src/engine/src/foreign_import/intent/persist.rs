//! Target-local exact-save intent records. No cleanup, overwrite or resume grant.
use super::*;
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory, linux::FileIdentity};

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    record: IntentRecord,
    digest: String,
}

/// The verified bytes and their exact file/directory owners remain together.
/// A retry may synchronize this observation, never reopen a substitute as proof.
pub(super) struct ObservedIntent {
    pub(super) record: IntentRecord,
    file: ReadFile,
    bytes: Vec<u8>,
    directory: File,
    parent: File,
    _activity: Option<DiskIoLease>,
}
impl ObservedIntent {
    fn check_names(&self, root: &DiskRoot) -> Result<()> {
        let directory = operation_path(root, self.record.operation_id);
        let named = root.open_read_regular(&directory.join("intent.json"))?;
        if FileIdentity::from_metadata(&named.metadata()?)
            != FileIdentity::from_metadata(&self.file.metadata()?)
            || root.directory_identity(&directory)? != FileIdentity::from_fd(&self.directory)?
            || root.directory_identity(&root.path().join(DIRECTORY))?
                != FileIdentity::from_fd(&self.parent)?
        {
            return Err(stale("foreign intent file or directory name changed"));
        }
        verify_record_bytes(&named, &self.bytes)
    }
    pub(super) fn synchronize(&self, root: &DiskRoot) -> Result<()> {
        self.check_names(root)?;
        self.file.sync_all()?;
        self.check_names(root)?;
        self.directory.sync_all()?;
        self.parent.sync_all()?;
        self.check_names(root)
    }
}

pub(super) fn read(root: &DiskRoot, operation: OperationId) -> Result<Option<ObservedIntent>> {
    let parent_path = root.path().join(DIRECTORY);
    if root.optional_metadata(&parent_path)?.is_none() {
        return Ok(None);
    }
    root.private_directory(&parent_path)?;
    let directory = operation_path(root, operation);
    let Some(metadata) = root.optional_metadata(&directory)? else {
        return Ok(None);
    };
    if !metadata.is_dir() {
        return Err(Error::new(
            ErrorCode::ObjectCollision,
            "foreign intent name is not its private operation directory",
        ));
    }
    let activity = root.retain_io_activity()?;
    root.private_directory(&directory)?;
    let held_directory = root.directory(&directory)?;
    let parent = root.directory(&root.path().join(DIRECTORY))?;
    let path = directory.join("intent.json");
    if root.optional_metadata(&path)?.is_none() {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "foreign import operation is retained without a complete intent; preserve it for recovery",
        ));
    }
    let file = root.open_read_regular(&path)?;
    let bytes = read_file_bytes(file.try_clone()?, MAX_INTENT_BYTES)?;
    let envelope: Envelope = decode_strict(&bytes)?;
    envelope.record.validate()?;
    if envelope.record.operation_id != operation
        || envelope.record.digest()?.to_string() != envelope.digest
    {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "foreign intent filename, content or digest differs",
        ));
    }
    let observed = ObservedIntent {
        record: envelope.record,
        file,
        bytes,
        directory: held_directory,
        parent,
        _activity: activity,
    };
    observed.check_names(root)?;
    Ok(Some(observed))
}

pub(super) fn save(root: &DiskRoot, record: &IntentRecord, reserve: u64) -> Result<()> {
    if let Some(existing) = read(root, record.operation_id)? {
        if existing.record != *record {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "foreign intent identifier already retains different reviewed input",
            ));
        }
        return existing.synchronize(root);
    }
    let bytes = encode_metadata(
        &Envelope {
            record: record.clone(),
            digest: record.digest()?.to_string(),
        },
        MAX_INTENT_BYTES,
    )?;
    let (operations, retained) = retained_usage(root)?;
    let charge = (bytes.len() as u64).checked_add(4096).ok_or_else(limit)?;
    if operations >= MAX_RETAINED_INTENTS
        || retained.checked_add(charge).ok_or_else(limit)? > MAX_RETAINED_BYTES
    {
        return Err(limit());
    }
    let required = charge.checked_add(reserve).ok_or_else(limit)?;
    if root.available()? < required {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "foreign intent would consume the target metadata reserve",
        ));
    }
    // Only the common control directory may be adopted. An operation directory
    // is created exclusively; an interrupted operation is never overwritten.
    let parent_path = root.path().join(DIRECTORY);
    root.private_dir(&parent_path)?;
    let parent = root.directory(&parent_path)?;
    let name = EntryName::new(record.operation_id.to_string())?;
    match rustix::fs::mkdirat(
        &parent,
        name.as_os_str(),
        rustix::fs::Mode::from_raw_mode(0o700),
    ) {
        Ok(()) => (),
        Err(rustix::io::Errno::EXIST) => {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "foreign operation directory appeared concurrently; preserve and inspect the same operation",
            ));
        }
        Err(error) => return Err(std::io::Error::from(error).into()),
    }
    let created = || -> Result<()> {
        parent.sync_all()?;
        let directory = operation_path(root, record.operation_id);
        let held_directory = root.directory(&directory)?;
        let private = PrivateDirectory::from_directory(held_directory.try_clone()?, directory)?;
        let published = AtomicFile::stage(&private, &EntryName::new("intent.json")?)?
            .prepare(&bytes)?
            .create()
            .map_err(|error| fs_error(error.into_source()))?;
        let observed = ObservedIntent {
            record: record.clone(),
            file: published.into(),
            bytes: bytes.clone(),
            directory: held_directory,
            parent: parent.try_clone()?,
            _activity: root.retain_io_activity()?,
        };
        observed.synchronize(root)?;
        if root.available()? < reserve {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "metadata reserve changed while saving foreign intent",
            ));
        }
        Ok(())
    };
    created().map_err(|error| {
        error.context(
            ErrorCode::RecoveryRequired,
            "foreign intent save may be retained; inspect this original operation before retrying",
        )
    })
}

fn operation_path(root: &DiskRoot, operation: OperationId) -> PathBuf {
    root.path().join(DIRECTORY).join(operation.to_string())
}

/// Bounded traversal accounts for every retained byte, including interrupted
/// atomic writes and directories without a complete intent. It never removes them.
fn retained_usage(root: &DiskRoot) -> Result<(usize, u64)> {
    let parent = root.path().join(DIRECTORY);
    if root.optional_metadata(&parent)?.is_none() {
        return Ok((0, 0));
    }
    root.private_directory(&parent)?;
    let mut operations = 0usize;
    let mut total = 0u64;
    let mut files = 0usize;
    for child in root.entries(&parent)? {
        operations += 1;
        if operations > MAX_RETAINED_INTENTS {
            return Err(limit());
        }
        let path = parent.join(child?.as_path());
        root.private_directory(&path)?;
        total = total.checked_add(4096).ok_or_else(limit)?;
        for child in root.entries(&path)? {
            files += 1;
            if files > MAX_RETAINED_INTENTS * 64 {
                return Err(limit());
            }
            let file = root.open_read_regular(&path.join(child?.as_path()))?;
            total = total
                .checked_add(file.metadata()?.len())
                .ok_or_else(limit)?;
            if total > MAX_RETAINED_BYTES {
                return Err(limit());
            }
        }
    }
    Ok((operations, total))
}
fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "retained foreign import intent budget exceeded",
    )
}
