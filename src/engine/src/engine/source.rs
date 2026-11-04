//! Explicit source provenance for copies prepared inside the engine.
use crate::{
    DiskFile, Result,
    disk::{DiskHandle, open_regular},
};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::FileExt,
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

    /// Every managed copy/sealing source retains its producer's I/O ownership.
    /// This entry point must not be a raw-file alternative to leased reads.
    pub(crate) fn open(&self) -> Result<ReadFile> {
        self.open_leased()
    }

    /// Keep the producer's disk ownership through every subsequent read. Local
    /// workspace/package inputs do not acquire an unrelated managed-disk gate.
    pub(crate) fn open_leased(&self) -> Result<ReadFile> {
        match self {
            Self::External(path) => open_regular(path).map(ReadFile::from),
            Self::Disk { owner, path } => owner.root.open_leased_regular(path).map(ReadFile::from),
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

/// An opened read source with explicit local or managed-disk ownership.
/// Managed resources never expose their inner file or an unleased descriptor.
pub struct ReadFile(ReadFileSource);

enum ReadFileSource {
    Local(File),
    Managed(DiskFile),
}

impl From<File> for ReadFile {
    fn from(file: File) -> Self {
        Self(ReadFileSource::Local(file))
    }
}

impl From<DiskFile> for ReadFile {
    fn from(file: DiskFile) -> Self {
        Self(ReadFileSource::Managed(file))
    }
}

impl ReadFile {
    pub fn check_running(&self) -> Result<()> {
        match &self.0 {
            ReadFileSource::Local(_) => Ok(()),
            ReadFileSource::Managed(file) => file.check_running(),
        }
    }

    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        match &self.0 {
            ReadFileSource::Local(file) => file.metadata(),
            ReadFileSource::Managed(file) => file.metadata(),
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        match &self.0 {
            ReadFileSource::Local(file) => file.sync_all(),
            ReadFileSource::Managed(file) => file.sync_all(),
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        match &self.0 {
            ReadFileSource::Local(file) => file.try_clone().map(Self::from),
            ReadFileSource::Managed(file) => file.try_clone().map(Self::from),
        }
    }
}

impl Read for ReadFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.0 {
            ReadFileSource::Local(file) => file.read(buffer),
            ReadFileSource::Managed(file) => file.read(buffer),
        }
    }
}

impl Seek for ReadFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match &mut self.0 {
            ReadFileSource::Local(file) => file.seek(position),
            ReadFileSource::Managed(file) => file.seek(position),
        }
    }
}

impl FileExt for ReadFile {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        match &self.0 {
            ReadFileSource::Local(file) => file.read_at(buffer, offset),
            ReadFileSource::Managed(file) => file.read_at(buffer, offset),
        }
    }

    fn write_at(&self, _: &[u8], _: u64) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "read source is not writable",
        ))
    }
}
