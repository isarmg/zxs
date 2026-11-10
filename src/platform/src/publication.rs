//! Descriptor-anchored atomic publication and retained private file bundles.
//! This module owns filesystem durability only, never product authority.
mod bundle;
use crate::fs_safety::{
    AtomicCommitError, EntryName,
    linux::{FileIdentity, OpenAt2Root, SingleLinkRequirement},
};
pub use bundle::{BundleDestination, BundlePublication, BundleState};
use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags, openat, renameat, renameat_with, unlinkat};
use std::{
    fs::File,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn random_name() -> Result<String> {
    let mut bytes = [0u8; 32];
    system_entropy::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(crate::lower_hex(&bytes))
}
/// Keeps the published inode alive until the caller finishes its readback.
/// This is a filesystem receipt, not content validation or product authority.
pub struct PublicationReceipt {
    identity: FileIdentity,
    _file: File,
}
impl PublicationReceipt {
    pub fn identity(&self) -> FileIdentity {
        self.identity
    }
}

/// Names are relative to the held directory identity, not a reopened path.
#[derive(Debug)]
pub struct PublicationFailure {
    pub directory: FileIdentity,
    pub staging: String,
    pub target: std::path::PathBuf,
    pub identity: FileIdentity,
    pub source: AtomicCommitError,
}
impl std::fmt::Display for PublicationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "helper publication target {:?}, staging {:?}, directory {:?}, file {:?}",
            self.target, self.staging, self.directory, self.identity
        )
    }
}
impl std::error::Error for PublicationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Publish an already prepared directory under its final, non-replacing name.
/// The caller holds the parent and child owners, synchronizes the prepared child,
/// and validates the staging identity and product policy before calling.
///
/// # Errors
/// A rename failure retains both names with an unknown outcome. Failures after
/// rename retain the published phase and its identity or durability error.
/// This function never cleans up, changes ownership, or starts a service.
pub fn publish_directory(
    parent: &OpenAt2Root,
    staging: &EntryName,
    target: &EntryName,
    directory: &OpenAt2Root,
) -> Result<()> {
    use crate::fs_safety::Error;
    let failure = |source| PublicationFailure {
        directory: parent.identity(),
        staging: staging.as_os_str().to_string_lossy().into_owned(),
        target: target.as_path().to_owned(),
        identity: directory.identity(),
        source,
    };
    renameat_with(
        parent.directory(),
        staging.as_os_str(),
        parent.directory(),
        target.as_os_str(),
        RenameFlags::NOREPLACE,
    )
    .map_err(|error| failure(AtomicCommitError::Unknown(Error::Io(error.into()))))?;
    parent
        .verify_entry_identity(target, directory.identity())
        .map_err(|error| failure(AtomicCommitError::Published(error)))?;
    parent.directory().sync_all().map_err(|error| {
        failure(AtomicCommitError::Published(
            Error::PublishedDurabilityUnknown(error),
        ))
    })?;
    Ok(())
}

#[derive(Debug)]
struct CleanupFailure {
    operation: Box<dyn std::error::Error + Send + Sync>,
    cleanup: std::io::Error,
}
impl std::fmt::Display for CleanupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "staging cleanup also failed: {}", self.cleanup)
    }
}
impl std::error::Error for CleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.operation.as_ref())
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
            tracing::error!(%error, "helper staging cleanup during unwind failed");
        }
    }
}

pub fn write_atomic_at(
    root: &OpenAt2Root,
    target: &EntryName,
    bytes: &[u8],
    replace: bool,
) -> Result<()> {
    write_atomic_at_mode(root, target, bytes, replace, 0o600)
}

pub fn write_atomic_at_mode(
    root: &OpenAt2Root,
    target: &EntryName,
    bytes: &[u8],
    replace: bool,
    mode: u32,
) -> Result<()> {
    write_atomic_at_mode_receipt(root, target, bytes, replace, mode).map(|_| ())
}

/// Retain the exact published inode after the existing durable publication.
/// Failure semantics and preservation of uncertain names match `write_atomic_at_mode`.
pub fn write_atomic_at_mode_receipt(
    root: &OpenAt2Root,
    target: &EntryName,
    bytes: &[u8],
    replace: bool,
    mode: u32,
) -> Result<PublicationReceipt> {
    use crate::fs_safety::Error;
    let directory = root.directory();
    if replace {
        let existing = root.open_file(&target.as_relative(), false)?;
        if existing.metadata()?.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::UnsafePermissions(target.as_path().to_owned()).into());
        }
    }
    let name = format!(".zxs-helper-{}.tmp", random_name()?);
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
    let result: Result<FileIdentity> = (|| {
        file.write_all(bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        let identity = SingleLinkRequirement::check(&file)?;
        // A failed reply (including EEXIST) is not proof that rename did not
        // happen. From the syscall attempt onward, preserve both names.
        cleanup.armed = false;
        let renamed = if replace {
            renameat(
                directory,
                cleanup.name.as_str(),
                directory,
                target.as_os_str(),
            )
        } else {
            renameat_with(
                directory,
                cleanup.name.as_str(),
                directory,
                target.as_os_str(),
                RenameFlags::NOREPLACE,
            )
        };
        let failure = |source| PublicationFailure {
            directory: root.identity(),
            staging: cleanup.name.clone(),
            target: target.as_path().to_owned(),
            identity,
            source,
        };
        renamed.map_err(|error| failure(AtomicCommitError::Unknown(Error::Io(error.into()))))?;
        root.verify_entry_identity(target, identity)
            .map_err(|error| failure(AtomicCommitError::Published(error)))?;
        directory.sync_all().map_err(|error| {
            failure(AtomicCommitError::Published(
                Error::PublishedDurabilityUnknown(error),
            ))
        })?;
        Ok(identity)
    })();
    match result {
        Ok(identity) => Ok(PublicationReceipt {
            identity,
            _file: file,
        }),
        Err(operation) => match cleanup.cleanup() {
            Ok(()) => Err(operation),
            Err(cleanup) => Err(Box::new(CleanupFailure { operation, cleanup })),
        },
    }
}

#[cfg(test)]
#[path = "../../../test/platform/unit/publication_tests.rs"]
mod tests;
