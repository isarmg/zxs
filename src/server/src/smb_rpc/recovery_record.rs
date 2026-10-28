//! Process-local ownership of a completed active recovery-record replacement.
use super::*;
use platform_io::fs_safety::{EntryName, PrivateDirectory};

const MAX_CACHED_BYTES: usize = 16 * 1024;

#[cfg(not(test))]
use platform_io::fs_safety::AtomicFile;
#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/recovery_persist.rs"]
pub(super) mod persist;
#[cfg(test)]
use persist::AtomicFile;

#[derive(Default)]
pub(super) struct ActiveRecord {
    persisted: Option<PersistedRecord>,
}

struct PersistedRecord {
    file: File,
    stamp: Stamp,
    bytes: Box<[u8]>,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    links: u64,
    mode: u32,
    owner: (u32, u32),
}

impl Stamp {
    fn read(file: &File) -> Result<Self, ApiError> {
        let metadata = file.metadata().map_err(ApiError::io)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            links: metadata.nlink(),
            mode: metadata.mode(),
            owner: (metadata.uid(), metadata.gid()),
        })
    }
}

impl PersistedRecord {
    fn unchanged(&self, directory: &PrivateDirectory, name: &EntryName) -> Result<bool, ApiError> {
        use platform_io::fs_safety::Error;
        if Stamp::read(&self.file)? != self.stamp {
            return Ok(false);
        }
        match directory.verify_payload(name, &self.file) {
            Ok(()) => {}
            Err(Error::IdentityChanged(_)) => return Ok(false),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(crate::upload_fs_error(error)),
        }
        // The previous unconditional replacement also repaired corrupt record
        // bytes. Preserve that behavior even if metadata did not reveal damage.
        // Read only this record, bounded by its previously encoded length; the
        // identity/stamp checks bracket this read, not the mutable SMB payload.
        let bytes = match directory.read_bounded(name, self.bytes.len()) {
            Ok(bytes) => bytes,
            Err(Error::BudgetExceeded) => return Ok(false),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(crate::upload_fs_error(error)),
        };
        if bytes.as_slice() != self.bytes.as_ref() {
            return Ok(false);
        }
        match directory.verify_payload(name, &self.file) {
            Ok(()) => Ok(Stamp::read(&self.file)? == self.stamp),
            Err(Error::IdentityChanged(_)) => Ok(false),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(crate::upload_fs_error(error)),
        }
    }
}

impl ActiveRecord {
    pub(super) fn persist(
        &mut self,
        directory: &PrivateDirectory,
        name: &EntryName,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError> {
        // Taking ownership also invalidates the old proof on every error. Exact
        // bytes cover workspace, operation, dirty/published and all other fields;
        // a dirty bool alone never grants permission to omit persistence.
        let previous = self.persisted.take();
        if let Some(previous) = previous
            && previous.bytes.as_ref() == bytes.as_slice()
            && previous.unchanged(directory, name)?
        {
            self.persisted = Some(previous);
            return Ok(());
        }
        directory
            .verify_identity()
            .map_err(crate::upload_fs_error)?;
        let file = AtomicFile::replace_held(directory, &name.as_relative(), &bytes)
            .map_err(crate::upload_fs_error)?;
        // Oversized records keep the original persistence path, not an expanded
        // in-memory allowance or a new disk-format rejection.
        if bytes.len() > MAX_CACHED_BYTES {
            return Ok(());
        }
        let stamp = Stamp::read(&file)?;
        self.persisted = Some(PersistedRecord {
            file,
            stamp,
            bytes: bytes.into_boxed_slice(),
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/recovery_owner_tests.rs"]
mod tests;
