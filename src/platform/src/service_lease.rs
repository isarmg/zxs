//! Lifetime exclusion on a stable, administratively owned directory inode.
//! This coordinates participating processes, not arbitrary root processes,
//! older binaries, Samba passdb writers, or external device writers.
use crate::fs_safety::{
    Error as FilesystemError,
    linux::{FileIdentity, MountPolicy, OpenAt2Root},
};
use std::{
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{fs::MetadataExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, Weak},
};

/// One fixed host-wide fence for current ZXS executables and their offline
/// Engine consumers. It is not a storage root or an initialization marker.
pub const HOST_ROOT: &str = "/zxs";

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("service lease descriptor transfer failed")]
    Inheritance(#[source] std::io::Error),
    #[error("a service lease owner is already admitted in this process")]
    AlreadyEntered,
    #[error("only the fixed root-owned host lease can enter the process scope")]
    NotHost,
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

/// Shared ownership of one locked open-file description. The last local or
/// inherited holder closes it; dropping one share never unlocks another owner.
#[derive(Clone)]
pub struct ServiceLease {
    path: PathBuf,
    owner: u32,
    root: Arc<OpenAt2Root>,
}
struct Entered {
    path: PathBuf,
    owner: u32,
    root: Weak<OpenAt2Root>,
}
static ENTERED: Mutex<Option<Entered>> = Mutex::new(None);

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
    /// ownership policy, sharing the explicitly admitted process owner when it
    /// names the same root. This never creates directories or changes permissions.
    ///
    /// # Errors
    /// Returns typed open, permission, lock and identity failures.
    pub fn acquire(path: &Path, owner: u32) -> Result<Self, Error> {
        if let Some(lease) = Self::current().filter(|lease| lease.path == path) {
            if lease.owner != owner {
                return Err(Error::Permissions {
                    path: path.to_owned(),
                    owner,
                });
            }
            lease.recheck()?;
            return Ok(lease);
        }
        Self::acquire_directory(path, owner, Self::open(path)?)
    }

    /// Lock an already anchored administrative directory, retaining its FD.
    ///
    /// # Errors
    /// Rejects unsafe permissions, contention and a changed named identity.
    pub fn acquire_directory(path: &Path, owner: u32, root: OpenAt2Root) -> Result<Self, Error> {
        Self::permissions(&root, path, owner)?;
        root.directory().try_lock().map_err(|error| Error::Lock {
            path: path.to_owned(),
            source: match error {
                std::fs::TryLockError::WouldBlock => {
                    FilesystemError::AlreadyLocked("service lease".into())
                }
                std::fs::TryLockError::Error(error) => FilesystemError::Io(error),
            },
        })?;
        let lease = Self {
            path: path.to_owned(),
            owner,
            root: Arc::new(root),
        };
        lease.recheck()?;
        Ok(lease)
    }

    /// Keep the same locked inode after publishing its new name.
    ///
    /// # Errors
    /// Rejects a new name that does not denote the held directory or its owner.
    pub fn published_at(mut self, path: &Path) -> Result<Self, Error> {
        self.path = path.to_owned();
        self.recheck()?;
        Ok(self)
    }

    /// Admit the helper/maintenance owner once, before any operation or runtime.
    /// The caller retains the returned owner through teardown or process exec.
    ///
    /// # Errors
    /// Rejects a non-host lease, changed root, or an existing process owner.
    pub fn enter_host(self) -> Result<Self, Error> {
        if self.path != Path::new(HOST_ROOT) || self.owner != 0 {
            return Err(Error::NotHost);
        }
        self.enter()
    }
    fn enter(self) -> Result<Self, Error> {
        self.recheck()?;
        let mut entered = ENTERED
            .lock()
            .expect("invariant: lease registry mutation cannot panic");
        if entered
            .as_ref()
            .and_then(|entry| entry.root.upgrade())
            .is_some()
        {
            return Err(Error::AlreadyEntered);
        }
        *entered = Some(Entered {
            path: self.path.clone(),
            owner: self.owner,
            root: Arc::downgrade(&self.root),
        });
        Ok(self)
    }
    fn current() -> Option<Self> {
        let entered = ENTERED
            .lock()
            .expect("invariant: lease registry mutation cannot panic");
        let entered = entered.as_ref()?;
        Some(Self {
            path: entered.path.clone(),
            owner: entered.owner,
            root: entered.root.upgrade()?,
        })
    }

    /// Share an admitted process owner only for this exact directory inode.
    ///
    /// # Errors
    /// Rechecks the admitted host name/permissions before granting a share.
    pub fn share_directory(root: &OpenAt2Root) -> Result<Option<Self>, Error> {
        let Some(lease) = Self::current() else {
            return Ok(None);
        };
        lease.recheck()?;
        Ok((lease.identity() == root.identity()).then_some(lease))
    }

    /// Borrow the anchored directory while this lease keeps its inode locked.
    pub fn root(&self) -> &OpenAt2Root {
        &self.root
    }

    /// Pass this lease only to the selected internal child. All parent FDs stay
    /// close-on-exec, including during ordinary external program execution.
    ///
    /// # Errors
    /// Returns a changed-root error before configuring the command. Descriptor
    /// inheritance failures are reported by Command::spawn/exec.
    #[allow(unsafe_code)]
    pub fn inherit_to(&self, command: &mut Command) -> Result<RawFd, Error> {
        self.recheck()?;
        let inherited = File::from(
            rustix::io::fcntl_dupfd_cloexec(self.root.directory(), 3)
                .map_err(|source| Error::Inheritance(source.into()))?,
        );
        let fd = inherited.as_raw_fd();
        // Keep the same open description above the child's standard I/O slots.
        // SAFETY: only fcntl runs after fork. The captured File retains this
        // valid FD until exec; changing its descriptor flags affects the child.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(inherited.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(fd)
    }

    /// Consume the explicit descriptor passed to an internal process entry.
    /// Normal daemon startup must call host(), never this function.
    ///
    /// # Safety
    /// Any open descriptor at `fd` must be exclusively owned by this startup
    /// boundary, with no existing Rust owner or borrowed handle in this process.
    ///
    /// # Errors
    /// Rejects standard/invalid FDs, non-directories, unsafe ownership, changed
    /// host identity, and independently opened descriptors unable to acquire it.
    #[allow(unsafe_code)]
    pub unsafe fn receive_host(fd: RawFd) -> Result<Self, Error> {
        // SAFETY: caller transfers sole ownership; descriptor validity is checked
        // before creating its File owner. fcntl accepts an arbitrary integer FD.
        let file = unsafe { Self::receive_file(fd)? };
        let path = Path::new(HOST_ROOT);
        let root = OpenAt2Root::from_directory(file, MountPolicy::AllowNestedMounts).map_err(
            |source| Error::Open {
                path: path.to_owned(),
                source,
            },
        )?;
        Self::acquire_directory(path, 0, root)
    }
    #[allow(unsafe_code)]
    unsafe fn receive_file(fd: RawFd) -> Result<File, Error> {
        if fd < 3 {
            return Err(Error::Inheritance(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "standard I/O is not a service lease",
            )));
        }
        // SAFETY: fcntl checks the descriptor without dereferencing user memory.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(Error::Inheritance(std::io::Error::last_os_error()));
        }
        // SAFETY: the caller transferred this now-validated open descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
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
