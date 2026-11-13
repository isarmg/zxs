//! The caller supplies permanent successor authority; this module supplies exact
//! original intent/body admission and the same store lock used by every reader.
use crate::disk::DiskHandle;
use crate::engine::container_drafts::store::{
    DraftLimits, Intent, MAX_INTENT, Result, StoreError, directory, engine,
    reopen_at, require_intent, require_limits,
};
use crate::persist::layout::container_drafts as layout;
use platform_io::fs_safety::linux::FileIdentity;
use storage_contract::FileId;
use storage_contract::container_edit::DraftRevision;

pub(crate) fn retire_superseded(
    disk: &DiskHandle,
    file: FileId,
    permits: impl Fn(&DraftRevision) -> crate::Result<bool>,
) -> Result<()> {
    let store = disk.root.path().join(layout::ROOT.as_path());
    if disk.root.optional_metadata(&store).map_err(engine)?.is_none() {
        return Ok(());
    }
    let root = disk.root.private_child(&store).map_err(engine)?;
    let directory_handle = root.directory(root.path()).map_err(engine)?;
    let _lock = platform_io::fs_safety::linux::AdvisoryLock::directory(&directory_handle)
        .map_err(|error| engine(error.into()))?;
    let parent = store.join(file.to_string());
    if root.optional_metadata(&parent).map_err(engine)?.is_none() {
        return Ok(());
    }
    let file_root = root.private_child(&parent).map_err(engine)?;
    for entry in file_root.entries(&parent).map_err(engine)? {
        let entry = entry.map_err(|error| engine(error.into()))?;
        let operation = entry.as_os_str().to_str().ok_or(StoreError::Incomplete)?
            .parse::<storage_contract::OperationId>().map_err(|_| StoreError::Incomplete)?;
        let path = parent.join(entry.as_path());
        let operation_root = root.private_child(&path).map_err(engine)?;
        let parent_identity = operation_root.directory_identity(&path).map_err(engine)?;
        let original = operation_root.read_bytes(&path.join(layout::INTENT.as_path()), MAX_INTENT)
            .map_err(engine)?;
        let intent: Intent = storage_contract::decode_strict(&original).map_err(StoreError::Codec)?;
        if intent.file != file || intent.revision.operation != operation
            || intent.expected != intent.revision.expected_generation
        {
            return Err(StoreError::Conflict);
        }
        let expected = Intent::new(disk, file, intent.expected, &intent.revision);
        require_intent(&operation_root, &path, &expected)?;
        if !permits(&intent.revision).map_err(engine)? {
            continue;
        }
        let body_path = path.join(layout::BODY.as_path());
        if operation_root.optional_metadata(&body_path).map_err(engine)?.is_none() {
            // A previous unlink may have completed before its directory sync.
            // Only this exact permanently superseded intent authorizes retry.
            if root.directory_identity(&path).map_err(engine)? != parent_identity {
                return Err(StoreError::Conflict);
            }
            require_intent(&operation_root, &path, &expected)?;
            // Never remove a body that appeared after the absence observation.
            operation_root.sync_dir(&path).map_err(engine)?;
            continue;
        }
        let body = operation_root.open_leased_regular(&body_path).map_err(engine)?;
        let body_identity = FileIdentity::from_metadata(
            &body.metadata().map_err(|error| engine(error.into()))?,
        );
        if !body.try_pin_retiring_draft().map_err(|error| engine(error.into()))? {
            continue;
        }
        body.verify_full(platform_io::payload::Expectation {
            bytes: intent.revision.size,
            blake3: Some(intent.revision.content_hash.digest()),
        }).map_err(StoreError::Payload)?;
        if root.directory_identity(&path).map_err(engine)? != parent_identity {
            return Err(StoreError::Conflict);
        }
        require_intent(&operation_root, &path, &expected)?;
        if !operation_root.remove_owned_regular_entry_synced(&layout::BODY, body_identity)
            .map_err(engine)?
        {
            return Err(StoreError::Conflict);
        }
        // The cleanup descriptor itself retains blocks after unlink. Close it
        // before releasing the store lock and allowing inventory to lower charge.
        drop(body);
    }
    Ok(())
}

pub(crate) fn verify_retired_or_present(
    disk: &DiskHandle,
    file: FileId,
    revision: &DraftRevision,
    limits: DraftLimits,
) -> Result<()> {
    require_limits(revision, limits)?;
    let path = directory(disk, file, revision);
    let root = disk.root.private_child(&disk.root.path().join(layout::ROOT.as_path()))
        .map_err(engine)?;
    let directory_handle = root.directory(root.path()).map_err(engine)?;
    let _lock = platform_io::fs_safety::linux::AdvisoryLock::directory(&directory_handle)
        .map_err(|error| engine(error.into()))?;
    require_intent(&root, &path,
        &Intent::new(disk, file, revision.expected_generation, revision))?;
    if root.optional_metadata(&path.join(layout::BODY.as_path())).map_err(engine)?.is_some() {
        // A corrupt retained body is not silently reclassified as retired.
        reopen_at(&root, disk, file, revision.expected_generation, revision,
            limits.checksum_chunk_bytes)?;
    }
    Ok(())
}
