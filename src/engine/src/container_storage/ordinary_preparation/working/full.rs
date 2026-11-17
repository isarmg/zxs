//! Explicit-commit materialization from one selected published working source.
//! The independent inode is private preparation, never a Version or saved draft.
use super::{HeldUnbornReader, ReadWorkingDigest, changed};
use crate::business::file_draft::DraftError as Failure;
use crate::disk::NewEntryCopyError;
use crate::engine::file_draft::SelectedWorkingSources;
use crate::engine::{PayloadProofReservation, PlanningContext};
use crate::file_stamp::FileStamp;
use crate::persist::layout::container::{self as layout, WorkingFullId};
use crate::{DiskFile, Error, ReadFile, Result};
use platform_io::fs_safety::EntryName;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_contract::ContentHash;
use storage_contract::file_draft::manifest::WORKING_BLOCK_BYTES;

enum FullSelection {
    Ordinary(Arc<SelectedWorkingSources>),
    Worktree(Arc<crate::engine::file_draft::worktree::WorktreeFirstSource>),
}
impl FullSelection {
    fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        match self {
            Self::Ordinary(selected) => selected.require_context(context),
            Self::Worktree(selected) => selected.require_context(context),
        }
    }
}

struct Merged<'a> {
    source: &'a HeldUnbornReader,
    offset: u64,
}

impl Read for Merged<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output.len().min(WORKING_BLOCK_BYTES as usize);
        let read = self.source.read_at(&mut output[..count], self.offset)
            .map_err(io::Error::other)?;
        self.offset += read as u64;
        Ok(read)
    }
}

/// Only prepare below can create this value. It retains the complete selected
/// CAS occurrence, original merged reader, independent full inode and its budget.
/// Abandoned preparation remains isolated and is never adopted by a pathname.
pub(crate) struct PreparedUnbornFull {
    selection: FullSelection,
    id: WorkingFullId,
    source: Arc<HeldUnbornReader>,
    name: EntryName,
    path: PathBuf,
    file: DiskFile,
    stamp: FileStamp,
    hash: ContentHash,
    // The final returned ReadFile also retains this product, not just its disk.
    _reservation: PayloadProofReservation,
}

impl PreparedUnbornFull {
    pub(crate) fn prepare(
        context: &PlanningContext<'_>,
        selection: Arc<SelectedWorkingSources>,
        selected_index: usize,
    ) -> Result<Arc<Self>> {
        Self::prepare_ordinary(context, selection, selected_index, None)
    }

    pub(super) fn prepare_ordinary(
        context: &PlanningContext<'_>, selection: Arc<SelectedWorkingSources>,
        selected_index: usize, measured: Option<(ContentHash, PayloadProofReservation)>,
    ) -> Result<Arc<Self>> {
        context.require_write()?;
        selection.require_context(context)?;
        let selected = selection.sources().get(selected_index)
            .ok_or(Error::FileDraft(Failure::WorkingSelection))?;
        if selected.target().path.parent().is_some() {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        let source = Arc::clone(selected.unborn_reader()?);
        source.require_record(selected.target().file, selected.record())?;
        Self::prepare_source(context, FullSelection::Ordinary(selection), source, measured)
    }

    pub(crate) fn prepare_worktree(
        context: &PlanningContext<'_>,
        selection: Arc<crate::engine::file_draft::worktree::WorktreeFirstSource>,
    ) -> Result<Arc<Self>> {
        selection.require_context(context)?;
        let source = Arc::clone(selection.reader());
        Self::prepare_source(context, FullSelection::Worktree(selection), source, None)
    }

    fn prepare_source(
        context: &PlanningContext<'_>, selection: FullSelection, source: Arc<HeldUnbornReader>,
        measured: Option<(ContentHash, PayloadProofReservation)>,
    ) -> Result<Arc<Self>> {
        context.require_write()?;
        selection.require_context(context)?;
        let disk = context.config.disks.iter().find(|disk| {
            disk.disk_id == source.scope.disk.seal.disk_id
        }).ok_or(Error::FileDraft(Failure::SourceChanged))?;
        if disk.level != 1 || disk.upstream_slot_id.is_some()
            || context.current_namespaces.is_some() || context.config.generation().is_some()
            || !Arc::ptr_eq(context.disk_resources.get(disk)?, &source.scope.disk)
        {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        let bytes = source.logical_size();
        if bytes > context.config.max_file_bytes {
            return Err(Error::FileDraft(Failure::Size));
        }
        // This isolated single-owner path needs this independent temporary plus
        // the ordinary immutable Version copy. It cannot spend only delta bytes.
        let required = bytes.checked_mul(2).ok_or(Error::FileDraft(Failure::Size))?;
        let used = context.used_bytes(&disk.slot_id);
        let available = source.scope.disk.root.available()?;
        if required.saturating_add(context.config.metadata_reserve_bytes) > available
            || disk.capacity_bytes.is_some_and(|limit| required > limit.saturating_sub(used))
        {
            return Err(Error::FileDraft(Failure::WorkingCapacity));
        }
        let (expected, reservation) = match measured {
            Some((hash, reservation)) => (Some(hash.digest()), reservation),
            None => (None, crate::engine::payload_proof_budget().reserve(2)
                .ok_or(Error::FileDraft(Failure::Size))?),
        };
        let id = WorkingFullId::generate()?;
        let name = layout::working_full(id);
        let scope = &source.scope;
        scope.verify()?;
        crate::container_storage::version_names::check_target_name(&scope.held, &name)?;
        let receipt = scope.held.copy_new_entry_verified_synced(
            &name,
            &mut Merged { source: &source, offset: 0 },
            platform_io::payload::Expectation { bytes, blake3: expected },
        ).map_err(|error| match error {
            NewEntryCopyError::Admission(source) | NewEntryCopyError::Published { source, .. } => {
                source
            }
            NewEntryCopyError::Payload(source) => source.into(),
        })?;
        let path = scope.held.path().join(name.as_path());
        let mut file = scope.held.open_leased_regular(&path)?;
        let metadata = file.metadata()?;
        if metadata.dev() != receipt.identity.device || metadata.ino() != receipt.identity.inode
            || metadata.nlink() != 1 || metadata.len() != bytes || receipt.bytes != bytes
        {
            return Err(changed());
        }
        let stamp = FileStamp::from_metadata(&metadata);
        // Reopen/readback is the physical boundary, not trust in a copied hash.
        file.verify_full(platform_io::payload::Expectation {
            bytes, blake3: Some(receipt.blake3),
        })?;
        selection.require_context(context)?;
        let result = Arc::new(Self {
            selection, id, source, name, path, file, stamp,
            hash: ContentHash::from_digest(receipt.blake3), _reservation: reservation,
        });
        result.require_context(context)?;
        Ok(result)
    }

    pub(crate) fn worktree_evidence(
        &self,
    ) -> Result<crate::persist::snapshot_worktree::added_commit::AddedFull> {
        if !matches!(self.selection, FullSelection::Worktree(_)) { return Err(changed()); }
        self.require_source()?;
        let metadata = self.file.metadata()?;
        Ok(crate::persist::snapshot_worktree::added_commit::AddedFull {
            id: self.id,
            entity: storage_contract::working_tree::LocalEntityIdentity::new(
                metadata.dev(), metadata.ino(),
            ),
        })
    }

    /// Called only after an independently verified first FILE publication.
    /// No State-context check is fabricated after that publication advanced it.
    pub(crate) fn move_worktree_full(
        &self, target: &crate::disk::DiskRoot, name: &EntryName,
        material: &crate::working_tree::snapshot::SnapshotFile,
    ) -> std::result::Result<(), crate::working_tree::snapshot::SnapshotStoreError> {
        use crate::working_tree::snapshot::SnapshotStoreError;
        let evidence = self.worktree_evidence().map_err(SnapshotStoreError::Disk)?;
        if material.file_id != self.source.file() || material.size != self.source.logical_size()
            || material.content_hash != self.hash
        {
            return Err(SnapshotStoreError::Version);
        }
        self.source.scope.held.move_new_entry_verified_synced(
            &self.name, target, name, crate::disk::FrozenFileMove {
                bytes: material.size, content_hash: material.content_hash, entity: evidence.entity,
            },
        ).map_err(SnapshotStoreError::Move)?;
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path { &self.path }

    pub(crate) fn reader(&self) -> &Arc<HeldUnbornReader> { &self.source }

    pub(crate) fn require_selection(
        &self, selection: &Arc<SelectedWorkingSources>, index: usize,
    ) -> Result<()> {
        let selected = selection.sources().get(index).ok_or_else(changed)?;
        if !matches!(&self.selection, FullSelection::Ordinary(original)
            if Arc::ptr_eq(original, selection))
            || !Arc::ptr_eq(&self.source, selected.unborn_reader()?)
        {
            return Err(changed());
        }
        self.require_source()
    }


    pub(crate) fn require_source(&self) -> Result<()> {
        self.source.require_source()?;
        self.file.check_running()?;
        let named = self.source.scope.held.open_read_entry(&self.name)?;
        if FileStamp::from_metadata(&self.file.metadata()?) != self.stamp
            || FileStamp::from_metadata(&named.metadata()?) != self.stamp
        {
            return Err(changed());
        }
        Ok(())
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        context.require_write()?;
        self.selection.require_context(context)?;
        self.require_source()
    }

    /// This digest came from the actual merged read and independent FULL
    /// readback. A manifest commitment can never construct this proof.
    pub(crate) fn digest(&self) -> Result<ReadWorkingDigest<'_>> {
        self.require_source()?;
        Ok(ReadWorkingDigest { source: &self.source, hash: self.hash })
    }

    pub(crate) fn open(self: &Arc<Self>, path: &Path) -> Result<ReadFile> {
        if path != self.path.as_path() { return Err(changed()); }
        self.require_source()?;
        let reservation = crate::engine::payload_proof_budget().reserve(1)
            .ok_or(Error::FileDraft(Failure::Size))?;
        let file = self.source.scope.held.open_leased_regular(path)?;
        if FileStamp::from_metadata(&file.metadata()?) != self.stamp {
            return Err(changed());
        }
        self.require_source()?;
        Ok(ReadFile::working_full(file, Arc::clone(self), reservation))
    }
}
