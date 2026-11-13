//! One held SYSTEM directory lock serializes every authority-format selection.
//! It is retained with writer ownership, never reconstructed from a cache.
mod fresh;
pub use fresh::FreshBootstrapError;
pub(crate) use fresh::{FreshSelection, has_fresh_selector};
pub(crate) const FRESH_SELECTOR: &str = fresh::SELECTOR;
pub(crate) const FRESH_DISK_SELECTOR: &str = "fresh-system-selection-v1.json";
use crate::{Error, ErrorCode, Result};
use platform_io::fs_safety::linux::{AdvisoryLock, MountPolicy, OpenAt2Root};
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use std::path::Path;

pub(crate) const ONLINE_SELECTION: &str = "authority-online.json";
pub(crate) const ONLINE_STATE: &str = "authority-online-state.json";

pub(crate) struct SelectionGuard {
    directory: PrivateDirectory,
    _lock: AdvisoryLock,
    fresh: Option<FreshSelection>,
}
impl SelectionGuard {
    pub(crate) fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_held(PrivateDirectory::open_existing(path)?)
    }
    pub(crate) fn acquire_held(directory: PrivateDirectory) -> Result<Self> {
        directory.verify_identity()?;
        let path = directory.path().to_owned();
        let root = OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)?;
        let lock = AdvisoryLock::directory(root.directory()).map_err(|source| {
            Error::from(source).context(
                ErrorCode::WriterAlreadyActive,
                "another runtime owns this SYSTEM authority selection",
            )
        })?;
        let directory = PrivateDirectory::from_directory(root.into_directory(), path.to_owned())?;
        directory.verify_identity()?;
        Ok(Self {
            directory,
            _lock: lock,
            fresh: None,
        })
    }
    pub(crate) fn legacy(path: &Path) -> Result<Self> {
        let guard = Self::acquire(path)?;
        guard.require_legacy()?;
        Ok(guard)
    }
    pub(crate) fn require_legacy(&self) -> Result<()> {
        require_legacy_directory(&self.directory)
    }
}

pub(crate) fn require_legacy(path: &Path) -> Result<()> {
    require_legacy_directory(&PrivateDirectory::open_existing(path)?)
}
fn require_legacy_directory(directory: &PrivateDirectory) -> Result<()> {
    // A missing selector never turns retained writer/fencing evidence into a
    // fresh legacy SYSTEM. Independent data roots cannot become SYSTEM either.
    for name in [
        ONLINE_SELECTION,
        ONLINE_STATE,
        "initialization.json",
        "identity.json",
        "initialized.json",
        "online-owner.json",
    ] {
        match directory.read_bounded(&EntryName::new(name)?, 64 * 1024) {
            Ok(_) => {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "SYSTEM retains independent disk authority; legacy pool recovery is forbidden",
                ));
            }
            Err(platform_io::fs_safety::Error::Io(source))
                if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(source.into()),
        }
    }
    directory.verify_identity()?;
    Ok(())
}
