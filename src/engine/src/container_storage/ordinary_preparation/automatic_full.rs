//! Independent immutable full synthesis for an already published Born save.
//! Unlike a spool path, the retained source is bound to this exact FILE and
//! controller through both the input and the verified independent output.
use super::ObservedContainerReplica;
use crate::business::file_draft::{DraftError, invalid};
use crate::disk::NewEntryCopyError;
use crate::engine::file_draft::SelectedWorkingSources;
use crate::engine::{PayloadProofReservation, PlanningContext};
use crate::file_stamp::FileStamp;
use crate::{DiskFile, Error, ReadFile, Result};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_contract::file_draft::{DraftLocation, DraftPayload};
use storage_contract::ContentHash;

pub(crate) struct PreparedAutomaticFull {
    selected: Arc<SelectedWorkingSources>,
    scope: Arc<ObservedContainerReplica>,
    path: PathBuf,
    file: DiskFile,
    stamp: FileStamp,
    _reservation: PayloadProofReservation,
}

impl PreparedAutomaticFull {
    pub(crate) fn prepare(
        context: &PlanningContext<'_>,
        selected: Arc<SelectedWorkingSources>,
        hash: ContentHash,
        mut checkpoint: impl FnMut() -> Result<()>,
    ) -> Result<Arc<Self>> {
        checkpoint()?;
        context.require_write()?;
        selected.require_context(context)?;
        let [source] = selected.sources() else { return Err(invalid()); };
        let scope = Arc::clone(source.born_scope()?);
        let (_, payload) = source.record().working.draft().ok_or_else(invalid)?;
        if source.record().working.automatic_controller().and_then(|c| c.pending_save()).is_none()
            || payload.content_hash().is_some_and(|expected| expected != hash)
        { return Err(invalid()); }
        let bytes = source.logical_size();
        let disk = context.config.disks.iter().find(|disk| disk.disk_id == source.record().owner_disk.to_string())
            .ok_or_else(invalid)?;
        if context.current_namespaces.is_some() || context.config.generation().is_some()
            || disk.level != 1 || disk.upstream_slot_id.is_some()
            || !Arc::ptr_eq(context.disk_resources.get(disk)?, &scope.disk)
        { return Err(invalid()); }
        let required = bytes.checked_mul(2).ok_or(Error::FileDraft(DraftError::Size))?;
        let disk_bytes = required.checked_add(context.config.metadata_reserve_bytes)
            .ok_or(Error::FileDraft(DraftError::Size))?;
        if bytes > context.config.max_file_bytes || disk_bytes > scope.disk.root.available()?
            || disk.capacity_bytes.is_some_and(|cap| required > cap.saturating_sub(context.used_bytes(&disk.slot_id)))
        { return Err(Error::FileDraft(DraftError::WorkingCapacity)); }
        let reservation = crate::engine::payload_proof_budget().reserve(3)
            .ok_or(Error::FileDraft(DraftError::Size))?;
        let name = crate::persist::layout::container::working_full(
            crate::persist::layout::container::WorkingFullId::generate()?);
        crate::container_storage::version_names::check_target_name(&scope.held, &name)?;
        let mut reader = selected.open_sole_born()?;
        let expectation = || platform_io::payload::Expectation { bytes, blake3: Some(hash.digest()) };
        let copied = scope.held.copy_new_entry_verified_synced_observed(
            &name, &mut reader, expectation(), &mut checkpoint,
        )
            .map_err(|e| match e {
                NewEntryCopyError::Admission(e) | NewEntryCopyError::Published { source: e, .. } => e,
                NewEntryCopyError::Payload(e) => e.into(),
            })?;
        let path = scope.held.path().join(name.as_path());
        let file = scope.held.open_leased_regular(&path)?;
        let metadata = file.metadata()?;
        if metadata.nlink() != 1 || metadata.len() != bytes || metadata.dev() != copied.identity.device
            || metadata.ino() != copied.identity.inode || copied.bytes != bytes || copied.blake3 != hash.digest()
        { return Err(invalid()); }
        file.verify_full_observed(expectation(), &mut checkpoint)?;
        checkpoint()?;
        selected.require_context(context)?;
        Ok(Arc::new(Self { selected, scope, path, file, stamp: FileStamp::from_metadata(&metadata),
            _reservation: reservation }))
    }
    pub(crate) fn path(&self) -> &Path { &self.path }
    pub(crate) fn require(&self, context: &PlanningContext<'_>, target: &DraftLocation, payload: DraftPayload) -> Result<()> {
        self.selected.require_context(context)?;
        let [source] = self.selected.sources() else { return Err(invalid()); };
        if source.target() != target || source.record().working.draft().map(|(_, p)| *p) != Some(payload) {
            return Err(invalid());
        }
        self.require_source()
    }
    pub(crate) fn require_source(&self) -> Result<()> {
        self.scope.verify()?;
        self.file.check_running()?;
        let actual = self.scope.held.open_leased_regular(&self.path)?;
        if FileStamp::from_metadata(&self.file.metadata()?) != self.stamp
            || FileStamp::from_metadata(&actual.metadata()?) != self.stamp { return Err(invalid()); }
        Ok(())
    }
    pub(crate) fn open(self: &Arc<Self>, path: &Path) -> Result<ReadFile> {
        if path != self.path { return Err(invalid()); }
        self.require_source()?;
        let reservation = crate::engine::payload_proof_budget().reserve(1)
            .ok_or(Error::FileDraft(DraftError::Size))?;
        let file = self.scope.held.open_leased_regular(path)?;
        if FileStamp::from_metadata(&file.metadata()?) != self.stamp { return Err(invalid()); }
        Ok(ReadFile::automatic_full(file, Arc::clone(self), reservation))
    }
}
