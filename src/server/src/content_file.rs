//! Transport-owned content without an unguarded managed-disk escape hatch.
use std::{
    fs::{File, Metadata},
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::FileExt,
};

/// Non-core backends may supply local files. Engine-backed content always keeps
/// the concrete disk owner, including while an HTTP body or SMB handle is idle.
pub struct ContentFile(Source);

enum Source {
    Local(File),
    #[cfg(feature = "core")]
    Engine(storage_engine::ReadFile),
}

impl From<File> for ContentFile {
    fn from(file: File) -> Self {
        Self(Source::Local(file))
    }
}

#[cfg(feature = "core")]
impl From<storage_engine::ReadFile> for ContentFile {
    fn from(file: storage_engine::ReadFile) -> Self {
        Self(Source::Engine(file))
    }
}

#[cfg(feature = "core")]
impl From<storage_engine::DiskFile> for ContentFile {
    fn from(file: storage_engine::DiskFile) -> Self {
        Self::from(storage_engine::ReadFile::from(file))
    }
}

impl ContentFile {
    /// Checks cancellation without touching the managed disk. This also applies
    /// to cached SMB chunks, which must not revive a fully cancelled handle.
    pub(crate) fn check_running(&self) -> io::Result<()> {
        match &self.0 {
            Source::Local(_) => Ok(()),
            #[cfg(feature = "core")]
            Source::Engine(file) => file
                .check_running()
                .map_err(|source| io::Error::new(io::ErrorKind::PermissionDenied, source)),
        }
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        match &self.0 {
            Source::Local(file) => file.metadata(),
            #[cfg(feature = "core")]
            Source::Engine(file) => file.metadata(),
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        match &self.0 {
            Source::Local(file) => file.sync_all(),
            #[cfg(feature = "core")]
            Source::Engine(file) => file.sync_all(),
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        match &self.0 {
            Source::Local(file) => file.try_clone().map(Self::from),
            #[cfg(feature = "core")]
            Source::Engine(file) => file.try_clone().map(Self::from),
        }
    }
}

impl Read for ContentFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.0 {
            Source::Local(file) => file.read(buffer),
            #[cfg(feature = "core")]
            Source::Engine(file) => file.read(buffer),
        }
    }
}

impl Seek for ContentFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match &mut self.0 {
            Source::Local(file) => file.seek(position),
            #[cfg(feature = "core")]
            Source::Engine(file) => file.seek(position),
        }
    }
}

impl FileExt for ContentFile {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        match &self.0 {
            Source::Local(file) => file.read_at(buffer, offset),
            #[cfg(feature = "core")]
            Source::Engine(file) => file.read_at(buffer, offset),
        }
    }

    fn write_at(&self, _: &[u8], _: u64) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "content is not writable",
        ))
    }
}
