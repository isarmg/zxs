//! Explicit source provenance for copies prepared inside the engine.
use crate::{
    Result,
    disk::{DiskHandle, open_regular},
};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

/// A caller/package path is a fresh named-path input. A managed source carries
/// the disk selected by its producer; opening never infers ownership from text.
#[derive(Clone)]
pub(crate) enum ReadSource {
    External(PathBuf),
    Disk {
        owner: Arc<DiskHandle>,
        path: PathBuf,
    },
}

impl ReadSource {
    pub(crate) fn external(path: impl Into<PathBuf>) -> Self {
        Self::External(path.into())
    }

    pub(crate) fn on_disk(owner: &Arc<DiskHandle>, path: PathBuf) -> Self {
        Self::Disk {
            owner: Arc::clone(owner),
            path,
        }
    }

    pub(crate) fn open(&self) -> Result<File> {
        match self {
            Self::External(path) => open_regular(path),
            Self::Disk { owner, path } => owner.root.open_regular(path),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::External(path) | Self::Disk { path, .. } => path,
        }
    }

    pub(super) fn join(&self, child: impl AsRef<Path>) -> Self {
        let path = self.path().join(child);
        match self {
            Self::External(_) => Self::External(path),
            Self::Disk { owner, .. } => Self::on_disk(owner, path),
        }
    }
}
