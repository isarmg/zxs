//! Lifetime exclusion on a stable, administratively owned directory inode.
//! This coordinates participating processes, not arbitrary root processes,
//! older binaries, Samba passdb writers, or external device writers.
use crate::fs_safety::{
    Error as FilesystemError,
    linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root},
};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// One fixed host-wide fence for current ZXS executables and their offline
/// Engine consumers. It is not a storage root or an initialization marker.
pub const HOST_ROOT: &str = "/zxs";

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("cannot open the existing service lease root {path:?}")]
    Open {
        path: PathBuf,
        #[source]
        source: FilesystemError,
    },
    #[error(
        "service lease root {path:?} must have owner UID {owner} and no group/other write access"
    )]
    Permissions { path: PathBuf, owner: u32 },
    #[error("service lease root {path:?} is busy or cannot be locked")]
    Lock {
        path: PathBuf,
        #[source]
        source: FilesystemError,
    },
    #[error("service lease root {path:?} changed identity while its old inode remained locked")]
    IdentityChanged { path: PathBuf },
    #[error("cannot inspect service lease root {path:?}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A non-cloneable OS lock. The caller must retain it through all admitted
/// worker and runtime teardown, not merely the end of an async request.
pub struct ServiceLease {
    path: PathBuf,
    owner: u32,
    root: OpenAt2Root,
    _lock: AdvisoryLock,
}
impl ServiceLease {
    /// Acquire the root-owned fixed current-daemon fence without creating it.
    ///
    /// # Errors
    /// Missing roots, links, unsafe ownership/modes, contention and unsupported
    /// kernel I/O retain their specific error. No lock file is created/deleted.
    pub fn host() -> Result<Self, Error> {
        Self::acquire(Path::new(HOST_ROOT), 0)
    }

    /// Acquire a caller-selected administrative directory under an explicit
    /// ownership policy. This does not create directories or change permissions.
    ///
    /// # Errors
    /// Returns typed open, permission, lock and identity failures.
    pub fn acquire(path: &Path, owner: u32) -> Result<Self, Error> {
        let root = Self::open(path)?;
        Self::permissions(&root, path, owner)?;
        let lock = AdvisoryLock::directory(root.directory()).map_err(|source| Error::Lock {
            path: path.to_owned(),
            source,
        })?;
        let lease = Self {
            path: path.to_owned(),
            owner,
            root,
            _lock: lock,
        };
        lease.recheck()?;
        Ok(lease)
    }
    fn open(path: &Path) -> Result<OpenAt2Root, Error> {
        OpenAt2Root::open(path, MountPolicy::AllowNestedMounts).map_err(|source| Error::Open {
            path: path.to_owned(),
            source,
        })
    }
    fn permissions(root: &OpenAt2Root, path: &Path, owner: u32) -> Result<(), Error> {
        let metadata = root
            .directory()
            .metadata()
            .map_err(|source| Error::Inspect {
                path: path.to_owned(),
                source,
            })?;
        if metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
            return Err(Error::Permissions {
                path: path.to_owned(),
                owner,
            });
        }
        Ok(())
    }
    /// The captured object identity, not a claim that a process is stopped.
    pub fn identity(&self) -> FileIdentity {
        self.root.identity()
    }

    /// Recheck the configured name at a caller's commit/use boundary. Root
    /// administration can rebind mounts, so holding an old inode alone does not
    /// permanently prove that a newly opened name still denotes it.
    ///
    /// # Errors
    /// Returns the original access failure or a typed identity/permission change.
    pub fn recheck(&self) -> Result<(), Error> {
        let current = Self::open(&self.path)?;
        if current.identity() != self.identity() {
            return Err(Error::IdentityChanged {
                path: self.path.clone(),
            });
        }
        Self::permissions(&current, &self.path, self.owner)
    }
}

#[cfg(test)]
#[path = "../../../test/platform/unit/service_lease/tests.rs"]
mod tests;
