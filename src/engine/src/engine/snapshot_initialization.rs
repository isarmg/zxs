//! Fresh directory initialization transaction. Intent admission is not publication.
//! The original intent must be retained before any repository material is created.
mod intent;
pub(crate) mod bootstrap;
pub(crate) mod registration;
mod store;
mod material;
mod execution;
pub(in crate::engine) use execution::RegisteredSnapshot;
pub use execution::{InitializedDirectoryEntry, InitializedDirectoryFile, InitializedDirectoryRead};

use super::Engine;
use crate::disk::DiskRoot;
use crate::{Error, ErrorCode, OperationId};
use platform_io::fs_safety::linux::FileIdentity;
use intent::AdmittedIntent;

#[derive(Debug, error_definition::Error)]
pub enum SnapshotInitializationError {
    #[error("directory snapshot source admission failed")]
    Source(#[source] super::directory_snapshot_source::DirectorySnapshotSourceError),
    #[error("directory snapshot material admission failed")]
    Material(#[source] material::MaterialError),
    #[error("directory snapshot registration admission failed")]
    Record(#[source] registration::RegistrationError),
    #[error("directory snapshot original intent admission failed")]
    Intent(#[source] intent::IntentError),
    #[error("directory snapshot identifier or metadata admission failed")]
    Contract(#[source] storage_contract::Error),
    #[error("directory snapshot publication does not close its original initialization")]
    Publication,
    #[error("directory initialization source container {0} has an uncommitted draft")]
    UncommittedSource(storage_contract::FileId),
    #[error("a retained directory initialization fences ordinary writes: {0}")]
    Pending(OperationId),
    #[error("directory initialization intent belongs to a different current pool or disk")]
    Registration,
    #[error("directory initialization original SYSTEM writer identity changed")]
    Writer,
    #[error("directory initialization intent replicas disagree")]
    Replicas,
    #[error("directory initialization intent storage failed")]
    Store(#[source] store::StoreError),
    #[error("directory initialization SYSTEM access failed")]
    System(#[source] Box<crate::Error>),
    #[error("directory initialization writer identity access failed")]
    Identity(#[source] std::io::Error),
}

impl SnapshotInitializationError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::UncommittedSource(_) => ErrorCode::UnsupportedOperation,
            Self::Pending(_) | Self::Store(_) | Self::System(_) => ErrorCode::RecoveryRequired,
            Self::Registration | Self::Writer | Self::Replicas | Self::Identity(_)
            | Self::Source(_) | Self::Material(_) | Self::Record(_) | Self::Intent(_)
            | Self::Contract(_) | Self::Publication => {
                ErrorCode::PublicationInvalid
            }
        }
    }
}

fn failure(error: SnapshotInitializationError) -> Error {
    Error::SnapshotInitialization(Box::new(error))
}

impl Engine {
    fn snapshot_intent_system(&self) -> Result<DiskRoot, SnapshotInitializationError> {
        let root = DiskRoot::open(&self.root)
            .map_err(|error| SnapshotInitializationError::System(Box::new(error)))?;
        let writer = root.open_regular_entry(&crate::persist::layout::CONTROL_WRITER_LOCK)
            .map_err(|error| SnapshotInitializationError::System(Box::new(error)))?;
        let observed = FileIdentity::from_fd(&writer)
            .map_err(SnapshotInitializationError::Identity)?;
        let expected = self._locks.system_identity()
            .map_err(|error| SnapshotInitializationError::System(Box::new(error)))?;
        if observed != expected {
            return Err(SnapshotInitializationError::Writer);
        }
        Ok(root)
    }

    /// Discovery installs only a deny fence. An intent is not a committed
    /// repository and cannot authorize adoption of prepared disk material.
    pub(in crate::engine) fn discover_snapshot_initialization(&mut self) -> crate::Result<()> {
        let root = self.snapshot_intent_system().map_err(failure)?;
        let mut original = store::read(&root)
            .map_err(|error| failure(SnapshotInitializationError::Store(error)))?;
        for disk in self.config.disks.iter().filter(|disk| disk.metadata_member) {
            let Some(handle) = self.disk_resources.available(disk) else {
                // Unavailability is not absence. Existing recovery already
                // fences missing members; do not manufacture an empty replica.
                continue;
            };
            let observed = store::read(&handle.root)
                .map_err(|error| failure(SnapshotInitializationError::Store(error)))?;
            if let Some(observed) = observed {
                if original.as_ref().is_some_and(|old| old.bytes() != observed.bytes()) {
                    return Err(failure(SnapshotInitializationError::Replicas));
                }
                original = Some(observed);
            }
        }
        if let Some(original) = original {
            if original.record().root.registration.pool_id != self.config.pool_id {
                return Err(failure(SnapshotInitializationError::Registration));
            }
            self.snapshot_initialization_intent = Some(original.record().operation);
        }
        Ok(())
    }

    pub(in crate::engine) fn require_no_snapshot_initialization(&self) -> crate::Result<()> {
        match self.snapshot_initialization_intent {
            Some(operation) => Err(failure(SnapshotInitializationError::Pending(operation))),
            None => Ok(()),
        }
    }

    /// No data material is created here. Even an uncertain SYSTEM save keeps
    /// the in-memory fence, and cold discovery retains the same original slot.
    fn retain_snapshot_initialization(&mut self, intent: &AdmittedIntent) -> crate::Result<()> {
        self.require_write()?;
        let record = intent.record();
        let mut members = Vec::new();
        for disk in self.config.disks.iter().filter(|disk| disk.metadata_member) {
            let registered = self.registered_disk(&disk.disk_id)
                .ok_or_else(|| failure(SnapshotInitializationError::Registration))?;
            if !registered.metadata_member
                || registered.registration_mode != super::RegistrationMode::Enabled
            {
                return Err(failure(SnapshotInitializationError::Registration));
            }
            members.push(registered.identity());
        }
        members.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
        if record.root.registration.pool_id != self.config.pool_id
            || members != record.metadata_members
            || self.snapshot.as_deref() != Some(record.source.publication.hash().to_string().as_str())
            || self.state.sequence != record.source.sequence.get()
        {
            return Err(failure(SnapshotInitializationError::Registration));
        }
        let root = self.snapshot_intent_system().map_err(failure)?;
        // A currently has one original initialization slot. Never fence a new
        // operation after discovering another retained original in that slot.
        if let Some(original) = store::read(&root)
            .map_err(|error| failure(SnapshotInitializationError::Store(error)))?
            && original.bytes() != intent.bytes()
        {
            return Err(failure(SnapshotInitializationError::Publication));
        }
        self.snapshot_initialization_intent = Some(intent.record().operation);
        store::save(&root, intent)
            .map_err(|error| failure(SnapshotInitializationError::Store(error)))?;
        for member in &intent.record().metadata_members {
            let registered = self.registered_disk(&member.disk_id)
                .ok_or_else(|| failure(SnapshotInitializationError::Registration))?;
            if registered.identity() != *member || !registered.metadata_member
                || registered.registration_mode != super::RegistrationMode::Enabled
            {
                return Err(failure(SnapshotInitializationError::Registration));
            }
            let disk = self.disk(&member.slot_id)?;
            let handle = self.disk_resources.get(disk)?;
            store::save(&handle.root, intent)
                .map_err(|error| failure(SnapshotInitializationError::Store(error)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_initialization_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_initialization_execution_tests.rs"]
mod execution_tests;
