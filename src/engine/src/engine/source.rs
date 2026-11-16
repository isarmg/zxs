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
    VerifiedReplica(Arc<super::reads::VerifiedReplicaSource>),
    Container {
        scope: Arc<crate::container_storage::ObservedContainerReplica>,
        version: crate::FileVersion,
        path: PathBuf,
    },
    VerifiedDraft {
        source: Arc<super::fresh_layout::draft_source::PublishedDraftSource>,
        path: PathBuf,
    },
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

    /// Location evidence only. Recovery still verifies the complete candidate
    /// bytes; this variant cannot satisfy the published backup-source gate.
    pub(crate) fn container(
        scope: Arc<crate::container_storage::ObservedContainerReplica>,
        version: &crate::FileVersion,
    ) -> Result<Self> {
        scope.require_version(version)?;
        let path = scope.body_path(version)?;
        Ok(Self::Container { scope, version: version.clone(), path })
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
            Self::VerifiedDraft { source, path } => source.open(path),
            Self::VerifiedReplica(source) => source.open(),
            Self::Container { scope, version, path } => {
                if path != &scope.body_path(version)? {
                    return Err(super::FreshLayoutError::ContainerScope.into());
                }
                let file = scope.container()?.open_leased_regular(path)?;
                scope.verify()?;
                Ok(ReadFile::in_container(file, Arc::clone(scope)))
            }
            Self::Disk { owner, path } => {
                owner.root_for(path)?.open_leased_regular(path).map(ReadFile::from)
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::External(path) | Self::Disk { path, .. }
            | Self::VerifiedDraft { path, .. } | Self::Container { path, .. } => path,
            Self::VerifiedReplica(source) => source.path(),
        }
    }

    pub(super) fn join(&self, child: impl AsRef<Path>) -> Self {
        let path = self.path().join(child);
        match self {
            Self::External(_) => Self::External(path),
            // Joining is not authority over another draft or a child path.
            // The retained source's exact-path gate rejects it on open.
            Self::VerifiedDraft { source, .. } => Self::VerifiedDraft {
                source: Arc::clone(source), path,
            },
            // Joining a child is not the original fully verified body path.
            Self::VerifiedReplica(source) => Self::VerifiedReplica(
                Arc::new(source.with_path(path)),
            ),
            Self::Container { scope, version, .. } => Self::Container {
                scope: Arc::clone(scope), version: version.clone(), path,
            },
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
    Container {
        file: DiskFile,
        scope: Arc<crate::container_storage::ObservedContainerReplica>,
    },
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
    /// Clone only into private scratch. The admitted disk keeps its I/O lease
    /// and the caller remains responsible for synchronization and publication.
    pub fn try_clone_working_file(&self, target: &File, bytes: u64) -> io::Result<bool> {
        match &self.0 {
            ReadFileSource::Local(file) => {
                platform_io::fs_safety::linux::try_clone_working_file(file, target, bytes)
            }
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. } => {
                file.try_clone_working_file(target, bytes)
            }
        }
    }

    /// Keep the admitted source chain alive as long as its returned BODY FD.
    pub(crate) fn in_container(
        file: DiskFile,
        scope: Arc<crate::container_storage::ObservedContainerReplica>,
    ) -> Self {
        Self(ReadFileSource::Container { file, scope })
    }

    pub(crate) fn verify_full(
        &mut self,
        expectation: platform_io::payload::Expectation,
    ) -> std::result::Result<platform_io::payload::Receipt, platform_io::payload::Error> {
        match &mut self.0 {
            ReadFileSource::Local(file) => platform_io::payload::verify_full(file, expectation),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. } => {
                file.verify_full(expectation)
            }
        }
    }

    pub fn check_running(&self) -> Result<()> {
        match &self.0 {
            ReadFileSource::Local(_) => Ok(()),
            ReadFileSource::Managed(file) => file.check_running(),
            ReadFileSource::Container { file, .. } => file.check_running(),
        }
    }

    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        match &self.0 {
            ReadFileSource::Local(file) => file.metadata(),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. } => {
                file.metadata()
            }
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        match &self.0 {
            ReadFileSource::Local(file) => file.sync_all(),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. } => {
                file.sync_all()
            }
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        match &self.0 {
            ReadFileSource::Local(file) => file.try_clone().map(Self::from),
            ReadFileSource::Managed(file) => file.try_clone().map(Self::from),
            ReadFileSource::Container { file, scope } => {
                file.try_clone().map(|file| Self::in_container(file, Arc::clone(scope)))
            }
        }
    }
}

impl Read for ReadFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.0 {
            ReadFileSource::Local(file) => file.read(buffer),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. } => {
                file.read(buffer)
            }
        }
    }
}

impl Seek for ReadFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match &mut self.0 {
            ReadFileSource::Local(file) => file.seek(position),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. } => {
                file.seek(position)
            }
        }
    }
}

impl FileExt for ReadFile {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        match &self.0 {
            ReadFileSource::Local(file) => file.read_at(buffer, offset),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. } => {
                file.read_at(buffer, offset)
            }
        }
    }

    fn write_at(&self, _: &[u8], _: u64) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "read source is not writable",
        ))
    }
}
