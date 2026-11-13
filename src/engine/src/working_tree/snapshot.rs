//! Immutable complete node material, separate from publication and branch authority.
//!
//! A prepared node is not a registered repository. Reopening requires the exact
//! binding from the caller's independently admitted permanent decision. Neither
//! directory names nor this module's manifests confer initialization authority.
mod manifest;
mod store;

pub(crate) use manifest::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile,
    SnapshotManifestError, SnapshotReference,
};
pub(crate) use store::{
    HeldSnapshotNode, SnapshotNodeBinding, SnapshotPreparationError, SnapshotStoreError,
};
