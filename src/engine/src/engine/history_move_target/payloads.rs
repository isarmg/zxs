//! Explicit full byte copies and original-copy inode binding; no hardlink,
//! reflink, payload deduplication or adoption of an unclaimed existing body.
use super::*;
use std::os::unix::fs::{FileExt, MetadataExt};

const IDENTITY: &str = "copy-identity.json";
const IDENTITY_BYTES: usize = 4096;

#[derive(Serialize)]
struct Identity<'a> {
    format: &'static str,
    version_id: &'a str,
    device: String,
    inode: String,
    bytes: String,
    modified_seconds: String,
    modified_nanoseconds: String,
    changed_seconds: String,
    changed_nanoseconds: String,
}

fn identity(file: &ReadFile, version: &FileVersion) -> Result<Vec<u8>> {
    let metadata = file.metadata()?;
    if metadata.nlink() != 1 || metadata.len() != version.logical_size {
        return Err(Error::new(ErrorCode::ObjectCollision, "move target must be one independent complete original copy"));
    }
    encode_metadata(&Identity {
        format: "file-history-move-original-full-copy",
        version_id: &version.version_id,
        device: metadata.dev().to_string(), inode: metadata.ino().to_string(),
        bytes: metadata.len().to_string(),
        modified_seconds: metadata.mtime().to_string(),
        modified_nanoseconds: metadata.mtime_nsec().to_string(),
        changed_seconds: metadata.ctime().to_string(),
        changed_nanoseconds: metadata.ctime_nsec().to_string(),
    }, IDENTITY_BYTES)
}

struct Source<'a> {
    file: &'a ReadFile,
    cancellation: &'a HistoryWindowCancellation,
}

impl FileExt for Source<'_> {
    fn read_at(&self, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
        checkpoint(self.cancellation).map_err(std::io::Error::other)?;
        let count = bytes.len().min(64 * 1024);
        self.file.read_at(&mut bytes[..count], offset)
    }
    fn write_at(&self, _: &[u8], _: u64) -> std::io::Result<usize> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "move source is read-only"))
    }
}

pub(super) fn prepare(
    engine: &Engine,
    target: &store::Target,
    original: &str,
    version: &FileVersion,
    cancellation: &HistoryWindowCancellation,
) -> Result<()> {
    checkpoint(cancellation)?;
    target.check_names()?;
    let root = &target.root;
    let directory = target.version_directory(version);
    let body = directory.join("content").join(&version.creation_name);
    let marker = directory.join(IDENTITY);
    let body_exists = crate::history_package::preparation_path_exists(root, &body)?;
    let marker_exists = crate::history_package::preparation_path_exists(root, &marker)?;
    if body_exists || marker_exists {
        if !body_exists || !marker_exists {
            return Err(Error::new(ErrorCode::RecoveryRequired, "partial move copy lacks its original body/identity pair; retained without adoption"));
        }
        verify(target, version, cancellation)?;
        root.open_regular(&body)?.sync_all()?;
        root.open_regular(&marker)?.sync_all()?;
        root.sync_dir(&directory)?;
        return target.check_names();
    }
    let _source_lease = engine.prepare_content(original)?;
    let source = engine.valid_backup_replica(original)?.open()?;
    root.private_dir(&directory.join("content"))?;
    crate::history_package::install_preparation_bytes(root, &directory.join("version.json"), &canonical_json(version)?)?;
    let copied = verified_full_copy_at(
        root,
        Source { file: &source, cancellation },
        &body,
        version,
        engine.config.max_file_bytes,
    ).map_err(CopyFailure::into_error)?;
    copied.check_at(root, &body)?;
    crate::history_package::install_preparation_bytes(
        root, &directory.join("checksums.bin"), &copied.checksums().encode()?,
    )?;
    let (_, held) = copied.into_parts();
    // Keep the actual copy FD through its identity publication. A matching
    // hash at an arbitrary old pathname cannot create this provenance.
    let original_identity = identity(&held, version)?;
    root.write_atomic(&marker, &original_identity, false)?;
    if identity(&held, version)? != original_identity
        || identity(&root.open_read_regular(&body)?, version)? != original_identity
    {
        return Err(Error::new(ErrorCode::PreparationStale, "new move copy changed before identity synchronization"));
    }
    root.sync_dir(&directory)?;
    verify(target, version, cancellation)?;
    target.check_names()
}

pub(super) fn verify(
    target: &store::Target,
    version: &FileVersion,
    cancellation: &HistoryWindowCancellation,
) -> Result<()> {
    checkpoint(cancellation)?;
    let root = &target.root;
    let directory = target.version_directory(version);
    let body = directory.join("content").join(&version.creation_name);
    let held = root.open_read_regular(&body)?;
    let stamp = identity(&held, version)?;
    if root.read_bytes(&directory.join(IDENTITY), IDENTITY_BYTES)? != stamp {
        return Err(Error::new(ErrorCode::PreparationStale, "move target body is no longer the original independently copied inode"));
    }
    crate::history_package::verify_package_payload_at(root, &directory, version, &mut |_| checkpoint(cancellation))?;
    if identity(&held, version)? != stamp
        || identity(&root.open_read_regular(&body)?, version)? != stamp
    {
        return Err(Error::new(ErrorCode::PreparationStale, "move target body changed during complete verification"));
    }
    target.check_names()
}
