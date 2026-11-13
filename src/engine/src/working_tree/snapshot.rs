//! Immutable complete node material, separate from publication and branch authority.
//!
//! A prepared node is not a registered repository. Reopening requires the exact
//! binding from the caller's independently admitted permanent decision. Neither
//! directory names nor this module's manifests confer initialization authority.
mod manifest;
mod store;

pub(crate) use manifest::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile,
    SnapshotFileSource, SnapshotManifestError, SnapshotPayload, SnapshotReference,
};
pub(crate) use store::{
    HeldSnapshotNode, SnapshotNodeBinding, SnapshotPreparationError, SnapshotStoreError,
};

/// A stable registered disk identity, not a device number, slot spelling, path
/// or content hash, defines the container-reference boundary of a repository.
#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
#[error(
    "directory repository on {repository_disk} cannot reference container {file} on {container_disk}"
)]
pub(crate) struct ContainerDiskError {
    pub(crate) repository_disk: storage_contract::DiskId,
    pub(crate) file: storage_contract::FileId,
    pub(crate) container_disk: storage_contract::DiskId,
}

/// The caller supplies both identities from admitted registration/placement
/// authority in one observation. Common and deleted locations share this rule.
pub(crate) fn require_container_disk(
    repository_disk: storage_contract::DiskId,
    file: storage_contract::FileId,
    container_disk: storage_contract::DiskId,
) -> Result<(), ContainerDiskError> {
    if repository_disk != container_disk {
        return Err(ContainerDiskError { repository_disk, file, container_disk });
    }
    Ok(())
}
