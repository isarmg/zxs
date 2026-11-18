//! Independently held SQLite half of one FILE material. A logical FILE
//! commitment is not its raw representation, physical bodies, lifecycles,
//! authority to install bytes, or coverage of any other registration.
pub(in crate::engine) mod clean_bodies;
pub(in crate::engine) mod census;

use super::{Representation, Schema, ScopeClosureError as Failure};
use crate::{
    DiskConfig, Result, State, ValidatedConfig,
    container_storage::{ObservedContainerReplica, file_database::ResidentFileImage},
    disk::DiskResources, engine::{PlanningContext, fresh_layout, membership_transition::PublishedBackupOriginal},
};
use std::sync::atomic::AtomicBool;
use storage_contract::{
    DiskId, FileId, HistoryTreeId, file_container::ContainerReplica,
    working_tree::{DiskRegistrationIdentity, MaterialCommitment},
};

pub(in crate::engine) struct ResidentFileRows {
    image: ResidentFileImage,
    holder: ObservedContainerReplica,
    registration: DiskRegistrationIdentity,
    replica: ContainerReplica,
    representation: Representation,
}
impl ResidentFileRows {
    /// This live source can be measured, not called an already-published
    /// byte commitment or an independent proof of the last-changing writer.
    pub(in crate::engine) fn current(
        context: &PlanningContext<'_>, disk: &DiskConfig, file: FileId,
    ) -> Result<Self> {
        Self::observe(context.config, context.disk_resources, context.state, disk, file)
    }
    fn observe(
        config: &ValidatedConfig, resources: &DiskResources, state: &State, disk: &DiskConfig, file: FileId,
    ) -> Result<Self> {
        let key = disk.disk_id.parse::<DiskId>()?;
        let root = config.storage_roots.get(&key).ok_or(Failure::PublicationMismatch)?;
        let record = state.file_containers.get(&file).ok_or(Failure::PublicationMismatch)?;
        let replica = record.replicas.get(&key).ok_or(Failure::PublicationMismatch)?;
        if fresh_layout::registration(config, disk) != root.registration || root.root_id != replica.root_id {
            return Err(Failure::PublicationMismatch.into());
        }
        // Native admission pins the complete source-prefix and physical
        // parent/registration, and the named container's original inode.
        let holder = fresh_layout::containers::observe_replica(config, resources, state, file, disk)?;
        fresh_layout::verify_root(config, disk, &holder.disk().root)?;
        let image = ResidentFileImage::read(&holder, file, record.history_tree_id)?;
        if image.sequence == 0 || image.sequence > state.sequence {
            return Err(Failure::PublicationMismatch.into());
        }
        let representation = Representation::new(Schema::File, image.bytes());
        image.require_named(&holder)?;
        Ok(Self { image, holder, registration: root.registration.clone(), replica: replica.clone(), representation })
    }
    pub(in crate::engine) fn registration(&self) -> &DiskRegistrationIdentity { &self.registration }
    pub(in crate::engine) fn replica(&self) -> &ContainerReplica { &self.replica }
    pub(in crate::engine) fn file(&self) -> FileId { self.image.file }
    pub(in crate::engine) fn history(&self) -> HistoryTreeId { self.image.history }
    pub(in crate::engine) fn source_sequence(&self) -> u64 { self.image.sequence }
    pub(in crate::engine) fn rows(&self) -> MaterialCommitment {
        MaterialCommitment::from_digest(self.image.records.digest())
    }
    pub(in crate::engine) fn representation(&self) -> Representation { self.representation }
    /// Keep the original holder and its FILE fd on both sides of consuming
    /// the bytes. The returned facts do not identify any unrelated BODY.
    pub(in crate::engine) fn with_image<T>(&self, read: impl FnOnce(&[u8]) -> Result<T>) -> Result<T> {
        self.image.require_named(&self.holder)?;
        let value = read(self.image.bytes())?;
        self.image.require_named(&self.holder)?;
        Ok(value)
    }
}

/// Only this exact Backup original also has a retained independent native
/// per-registration last-writer proof. Replaying arbitrary current State or
/// accepting any equal-row SQLite with a different epoch does not qualify.
pub(in crate::engine) fn backup_predecessor(
    source: &PublishedBackupOriginal<'_>, resources: &DiskResources, file: FileId, stop: &AtomicBool,
) -> Result<ResidentFileRows> {
    source.require(stop)?;
    let rows = ResidentFileRows::observe(source.config(), resources, source.original().state(), source.disk(), file)?;
    source.require_file_predecessor(file, rows.image.sequence, &rows.image.records.to_string())?;
    rows.image.require_named(&rows.holder)?;
    source.require(stop)?;
    Ok(rows)
}
