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
    OriginalBody {
        source: Arc<crate::snapshot_worktree::OriginalBody>,
        path: PathBuf,
    },
    Container {
        scope: Arc<crate::container_storage::ObservedContainerReplica>,
        version: crate::FileVersion,
        path: PathBuf,
    },
    WorkingFull {
        source: Arc<crate::container_storage::PreparedUnbornFull>,
        path: PathBuf,
    },
    SelectedWorking {
        selection: Arc<super::file_draft::SelectedWorkingSources>,
        index: usize,
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
    pub(crate) fn original_body(source: Arc<crate::snapshot_worktree::OriginalBody>) -> Self {
        let path = source.path().to_owned();
        Self::OriginalBody { source, path }
    }

    pub(crate) fn is_original_body(&self, sparse: &Arc<crate::directory_worktree::store::HeldSparseWorktree>,
        edit: &crate::directory_worktree::ledger::EditRecord) -> bool {
        matches!(self, Self::OriginalBody { source, path } if path == source.path() && source.is_exact(sparse, edit))
    }

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

    /// Only an actual independently copied FULL can enter the Version builder.
    /// A sparse working FD, manifest or raw path cannot construct this input.
    pub(crate) fn working_full(source: Arc<crate::container_storage::PreparedUnbornFull>) -> Self {
        let path = source.path().to_owned();
        Self::WorkingFull { source, path }
    }

    pub(crate) fn require_working_full(
        &self,
    ) -> Result<&Arc<crate::container_storage::PreparedUnbornFull>> {
        match self {
            Self::WorkingFull { source, path } if path == source.path() => Ok(source),
            _ => Err(crate::business::file_draft::invalid()),
        }
    }

    /// One source from the already admitted complete selected set. No raw
    /// path, independent singleton picker or reissued container can replace it.
    pub(crate) fn selected_working(
        selection: Arc<super::file_draft::SelectedWorkingSources>, index: usize,
    ) -> Result<Self> {
        let path = selection.born_path(index)?;
        Ok(Self::SelectedWorking { selection, index, path })
    }

    pub(crate) fn require_selected_working(
        &self, expected: &Arc<super::file_draft::SelectedWorkingSources>, index: usize,
    ) -> Result<()> {
        match self {
            Self::SelectedWorking { selection, index: actual, path }
                if Arc::ptr_eq(selection, expected) && *actual == index
                    && *path == selection.born_path(index)? => Ok(()),
            _ => Err(crate::business::file_draft::invalid()),
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
            Self::VerifiedDraft { source, path } => source.open(path),
            Self::OriginalBody { source, path } => source.open(path),
            Self::SelectedWorking { selection, index, path } => selection.open_born(*index, path),
            Self::WorkingFull { source, path } => source.open(path),
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
            Self::External(path) | Self::Disk { path, .. } | Self::OriginalBody { path, .. }
            | Self::VerifiedDraft { path, .. } | Self::Container { path, .. }
            | Self::WorkingFull { path, .. } | Self::SelectedWorking { path, .. } => path,
            Self::VerifiedReplica(source) => source.path(),
        }
    }

    pub(super) fn join(&self, child: impl AsRef<Path>) -> Self {
        let path = self.path().join(child);
        match self {
            Self::External(_) => Self::External(path),
            Self::OriginalBody { source, .. } => Self::OriginalBody {
                source: Arc::clone(source), path,
            },
            // Joining is not authority over another draft or a child path.
            // The retained source's exact-path gate rejects it on open.
            Self::WorkingFull { source, .. } => Self::WorkingFull {
                source: Arc::clone(source), path,
            },
            Self::SelectedWorking { selection, index, .. } => Self::SelectedWorking {
                selection: Arc::clone(selection), index: *index, path,
            },
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

impl ReadFile {
    /// A range identity is returned only for the actual engine reader that
    /// verifies its base checksum chunk or immutable extent before every read.
    pub fn verified_working_identity(&self)
        -> Result<Option<storage_contract::file_draft::DraftPayload>>
    {
        match &self.0 {
            ReadFileSource::IncrementalDraft { scope, .. } => scope.incremental_identity().map(Some),
            ReadFileSource::Local(_) | ReadFileSource::Managed(_)
            | ReadFileSource::WorkingFull { .. } | ReadFileSource::Container { .. } => Ok(None),
        }
    }
}

enum ReadFileSource {
    IncrementalDraft {
        scope: Arc<crate::container_storage::ObservedContainerReplica>,
        offset: u64,
    },
    Local(File),
    Managed(DiskFile),
    WorkingFull {
        file: DiskFile,
        source: Arc<crate::container_storage::PreparedUnbornFull>,
        _reservation: super::PayloadProofReservation,
    },
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
            ReadFileSource::IncrementalDraft { .. } => Ok(false),
            ReadFileSource::Local(file) => {
                platform_io::fs_safety::linux::try_clone_working_file(file, target, bytes)
            }
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. }
            | ReadFileSource::WorkingFull { file, .. } => {
                file.try_clone_working_file(target, bytes)
            }
        }
    }

    pub(crate) fn incremental_draft(
        scope: Arc<crate::container_storage::ObservedContainerReplica>,
    ) -> Self {
        Self(ReadFileSource::IncrementalDraft { scope, offset: 0 })
    }

    pub fn logical_size(&self) -> io::Result<u64> {
        match &self.0 {
            ReadFileSource::IncrementalDraft { scope, .. } => {
                scope.working_size().map_err(io::Error::other)
            }
            ReadFileSource::Local(_) | ReadFileSource::Managed(_)
            | ReadFileSource::WorkingFull { .. } | ReadFileSource::Container { .. } => {
                self.metadata().map(|metadata| metadata.len())
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

    pub(crate) fn working_full(
        file: DiskFile,
        source: Arc<crate::container_storage::PreparedUnbornFull>,
        reservation: super::PayloadProofReservation,
    ) -> Self {
        Self(ReadFileSource::WorkingFull { file, source, _reservation: reservation })
    }

    pub(crate) fn verify_full(
        &mut self,
        expectation: platform_io::payload::Expectation,
    ) -> std::result::Result<platform_io::payload::Receipt, platform_io::payload::Error> {
        match &mut self.0 {
            ReadFileSource::IncrementalDraft { .. } => Err(platform_io::payload::Error::Io(
                io::Error::new(io::ErrorKind::Unsupported,
                    "a merged working body has no independent FULL inode"),
            )),
            ReadFileSource::Local(file) => platform_io::payload::verify_full(file, expectation),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. }
            | ReadFileSource::WorkingFull { file, .. } => {
                file.verify_full(expectation)
            }
        }
    }

    pub fn check_running(&self) -> Result<()> {
        match &self.0 {
            ReadFileSource::IncrementalDraft { scope, .. } => scope.verify(),
            ReadFileSource::Local(_) => Ok(()),
            ReadFileSource::Managed(file) => file.check_running(),
            ReadFileSource::Container { file, .. } => file.check_running(),
            ReadFileSource::WorkingFull { file, .. } => file.check_running(),
        }
    }

    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        match &self.0 {
            ReadFileSource::IncrementalDraft { .. } => Err(io::Error::new(
                io::ErrorKind::Unsupported, "a merged working body has no single inode metadata",
            )),
            ReadFileSource::Local(file) => file.metadata(),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. }
            | ReadFileSource::WorkingFull { file, .. } => {
                file.metadata()
            }
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        match &self.0 {
            ReadFileSource::IncrementalDraft { .. } => Err(io::Error::new(
                io::ErrorKind::Unsupported, "immutable working blocks are already synchronized",
            )),
            ReadFileSource::Local(file) => file.sync_all(),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. }
            | ReadFileSource::WorkingFull { file, .. } => {
                file.sync_all()
            }
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        match &self.0 {
            ReadFileSource::IncrementalDraft { scope, offset } => {
                scope.verify().map_err(io::Error::other)?;
                Ok(Self(ReadFileSource::IncrementalDraft {
                    scope: Arc::clone(scope), offset: *offset,
                }))
            }
            ReadFileSource::Local(file) => file.try_clone().map(Self::from),
            ReadFileSource::Managed(file) => file.try_clone().map(Self::from),
            ReadFileSource::Container { file, scope } => {
                file.try_clone().map(|file| Self::in_container(file, Arc::clone(scope)))
            }
            ReadFileSource::WorkingFull { file, source, .. } => {
                let reservation = super::payload_proof_budget().reserve(1).ok_or_else(|| {
                    io::Error::other(crate::business::file_draft::DraftError::Size)
                })?;
                file.try_clone().map(|file| {
                    Self::working_full(file, Arc::clone(source), reservation)
                })
            }
        }
    }
}

impl Read for ReadFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.0 {
            ReadFileSource::IncrementalDraft { scope, offset } => {
                let count = scope.read_incremental_draft(buffer, *offset)
                    .map_err(io::Error::other)?;
                *offset += count as u64;
                Ok(count)
            }
            ReadFileSource::Local(file) => file.read(buffer),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. }
            | ReadFileSource::WorkingFull { file, .. } => {
                file.read(buffer)
            }
        }
    }
}

impl Seek for ReadFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match &mut self.0 {
            ReadFileSource::IncrementalDraft { scope, offset } => {
                scope.verify().map_err(io::Error::other)?;
                let next = match position {
                    SeekFrom::Start(position) => i128::from(position),
                    SeekFrom::Current(relative) => i128::from(*offset) + i128::from(relative),
                    SeekFrom::End(relative) => i128::from(scope.working_size()
                        .map_err(io::Error::other)?) + i128::from(relative),
                };
                *offset = u64::try_from(next).map_err(|source| {
                    io::Error::new(io::ErrorKind::InvalidInput, source)
                })?;
                Ok(*offset)
            }
            ReadFileSource::Local(file) => file.seek(position),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. }
            | ReadFileSource::WorkingFull { file, .. } => {
                file.seek(position)
            }
        }
    }
}

impl FileExt for ReadFile {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        match &self.0 {
            ReadFileSource::IncrementalDraft { scope, .. } => {
                scope.read_incremental_draft(buffer, offset).map_err(io::Error::other)
            }
            ReadFileSource::Local(file) => file.read_at(buffer, offset),
            ReadFileSource::Managed(file) | ReadFileSource::Container { file, .. }
            | ReadFileSource::WorkingFull { file, .. } => {
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
