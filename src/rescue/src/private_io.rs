//! Descriptor-anchored CLI files. Output parents may be public (for example
//! /tmp); files are owner-only and single-linked. Existing outputs are never
//! clobbered; draft replacements require a held transaction.
use crate::Result;
use platform_io::fs_safety::{
    EntryName,
    linux::{AdvisoryLock, MountPolicy, OpenAt2Root},
};
use rustix::fs::{
    AtFlags, Mode, OFlags, RenameFlags, openat, renameat, renameat_with, statat, unlinkat,
};
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

struct AnchoredPath {
    directory: OpenAt2Root,
    name: EntryName,
}
impl AnchoredPath {
    fn open(path: &Path) -> Result<Self> {
        let name = EntryName::new(path.file_name().ok_or("file name is required")?)?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // Resolve the caller-selected parent once. All subsequent file opens,
        // publication and cleanup use the held directory descriptor.
        let parent = parent.canonicalize()?;
        Ok(Self {
            directory: OpenAt2Root::open(&parent, MountPolicy::AllowNestedMounts)?,
            name,
        })
    }
}

/// Read an explicitly selected owned draft without following links. Drafts
/// need not have secret-file permissions, but still use the regular-file and
/// single-link checks of the descriptor-anchored I/O layer.
impl AnchoredPath {
    fn read_owned_bytes(&self, maximum: usize) -> Result<Vec<u8>> {
        let file = self.directory.open_file(&self.name.as_relative(), false)?;
        let metadata = file.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err("configuration draft must belong to this user".into());
        }
        if metadata.len() > maximum as u64 {
            return Err("configuration draft exceeds its byte limit".into());
        }
        let mut bytes = Vec::new();
        file.take((maximum as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > maximum {
            return Err("configuration draft exceeds its byte limit".into());
        }
        Ok(bytes)
    }
}
/// One draft read/validate/backup/replace transaction. Lock the stable parent,
/// not the draft inode that atomic publication replaces. Parent aliases share
/// the same lock, and overlapping backup names cannot bypass it. Contenders
/// fail before reading a snapshot; no sidecar is created or unlinked.
///
/// This is an advisory offline-tool lock, not a substitute for stopping the
/// daemon or excluding unrelated writers before editing a draft.
pub(crate) struct DraftTransaction {
    target: AnchoredPath,
    _lock: AdvisoryLock,
}
impl DraftTransaction {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let target = AnchoredPath::open(path)?;
        let lock = AdvisoryLock::directory(target.directory.directory()).map_err(|error| {
            match error {
                platform_io::fs_safety::Error::AlreadyLocked(_) => {
                    "another configuration draft edit is active in this directory; retry after it finishes".into()
                }
                other => Box::new(other) as Box<dyn std::error::Error + Send + Sync>,
            }
        })?;
        Ok(Self {
            target,
            _lock: lock,
        })
    }

    pub(crate) fn read_owned_bytes(&self, maximum: usize) -> Result<Vec<u8>> {
        self.target.read_owned_bytes(maximum)
    }

    /// Inspect markers and backup names relative to the same held parent as
    /// the draft. Never recanonicalize a parent that could have been rebound.
    pub(crate) fn entry_exists(&self, name: &OsStr) -> Result<bool> {
        let name = EntryName::new(name)?;
        match statat(
            self.target.directory.directory(),
            name.as_os_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn write_backup(&self, name: &OsStr, original: &[u8]) -> Result<()> {
        let replace = self.entry_exists(name)?;
        Ok(write_atomic_at(
            &self.target.directory,
            &EntryName::new(name)?,
            original,
            replace,
        )?)
    }

    pub(crate) fn replace_private(&self, bytes: &[u8]) -> Result<()> {
        Ok(write_atomic_at(
            &self.target.directory,
            &self.target.name,
            bytes,
            true,
        )?)
    }
}

struct StagingCleanup<'a> {
    directory: &'a File,
    name: String,
    armed: bool,
}
impl StagingCleanup<'_> {
    fn cleanup(&mut self) -> std::io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        match unlinkat(self.directory, self.name.as_str(), AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
impl Drop for StagingCleanup<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            tracing::error!(%error, "rescue staging cleanup during unwind failed");
        }
    }
}

/// Publish only a complete 0600 file; existing files, directories and links win.
/// A directory-sync failure after rename preserves PublishedDurabilityUnknown.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = AnchoredPath::open(path)?;
    Ok(write_atomic_at(
        &target.directory,
        &target.name,
        bytes,
        false,
    )?)
}

fn write_atomic_at(
    root: &OpenAt2Root,
    target: &EntryName,
    bytes: &[u8],
    replace: bool,
) -> std::result::Result<(), platform_io::fs_safety::Error> {
    use platform_io::fs_safety::Error;
    let directory = root.directory();
    if replace {
        let existing = root.open_file(&target.as_relative(), false)?;
        if existing.metadata()?.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::UnsafePermissions(target.as_path().to_owned()));
        }
    }
    let name = format!(
        ".zxs-rescue-{}.tmp",
        authentication::random_token().map_err(std::io::Error::other)?
    );
    let mut file = File::from(
        openat(
            directory,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?,
    );
    // Arm cleanup only after the exclusive inode was created successfully.
    let mut cleanup = StagingCleanup {
        directory,
        name,
        armed: true,
    };
    let result = (|| {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if replace {
            renameat(
                directory,
                cleanup.name.as_str(),
                directory,
                target.as_os_str(),
            )
            .map_err(std::io::Error::from)?;
        } else {
            renameat_with(
                directory,
                cleanup.name.as_str(),
                directory,
                target.as_os_str(),
                RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
        }
        // The temporary name no longer belongs to us after publication. Disarm
        // before syncing so a later user of that name cannot be removed by Drop.
        cleanup.armed = false;
        directory
            .sync_all()
            .map_err(platform_io::fs_safety::Error::PublishedDurabilityUnknown)?;
        Ok(())
    })();
    match result {
        Ok(value) => Ok(value),
        Err(operation) => match cleanup.cleanup() {
            Ok(()) => Err(operation),
            Err(cleanup) => Err(Error::Cleanup {
                operation: Box::new(operation),
                cleanup,
            }),
        },
    }
}

#[cfg(test)]
#[path = "../../../test/rescue/unit/private_io/tests.rs"]
mod tests;
