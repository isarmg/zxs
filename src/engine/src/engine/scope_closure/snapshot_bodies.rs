//! One complete directory node's physical payloads, expanded back to every
//! original manifest path. This is not a complete repository or disk scope.
use super::{OrdinalBuilder, OrdinalPages, ScopeClosureError as Failure};
use crate::{
    Error, Result,
    persist::layout::snapshot as layout,
    working_tree::snapshot::{
        AdmittedSnapshotManifest, HeldSnapshotNode, MAX_ENTRIES, SnapshotEntry, SnapshotEntryContent,
        SnapshotFile, SnapshotReference, SnapshotStoreError,
    },
};
use data_serialization::Serialize;
use platform_io::fs_safety::EntryName;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{ContentHash, OperationId, VersionId, working_tree::LocalEntityIdentity};

#[derive(Serialize)]
struct EntrySource<'a> {
    // Neither an unqualified path nor a FILE/version/hash alone is an
    // original directory occurrence. The native entry includes its selector.
    snapshot: SnapshotReference,
    entry: &'a SnapshotEntry,
}

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
enum Body<'a> {
    DirectorySnapshot {
        source: EntrySource<'a>,
        relative_name: &'a str,
        #[serialization(with = "storage_contract::u64_string")]
        bytes: u64,
        content_hash: ContentHash,
    },
}

/// Transient physical facts. A reserved move is not a presently readable
/// destination; admission of the eventual target must reopen its real leaf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::engine) enum OriginalLeaf {
    Present { entity: LocalEntityIdentity },
    ReservedMove {
        entity: LocalEntityIdentity,
        source_parent: LocalEntityIdentity,
        source_name: EntryName,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::engine) struct BodyLeaf {
    pub(in crate::engine) logical: SnapshotFile,
    pub(in crate::engine) operation: OperationId,
    pub(in crate::engine) name: EntryName,
    pub(in crate::engine) physical: OriginalLeaf,
}

/// Neither the material reference nor these original source identities give a
/// right to install a staged target, elect a frontier, or resurrect a FILE.
pub(in crate::engine) struct NodeBodies {
    pub(in crate::engine) reference: SnapshotReference,
    pub(in crate::engine) operation: OperationId,
    pub(in crate::engine) pages: OrdinalPages,
    // One physical body per VersionId within this exact node. The pages above
    // still keep each logical path occurrence independently.
    pub(in crate::engine) leaves: BTreeMap<VersionId, BodyLeaf>,
}
impl NodeBodies {
    /// A complete immutable existing node can use the native held owner now.
    /// Pending target moves have a different source adapter; never treat
    /// their not-yet-created target file as input to this read-only path.
    pub(in crate::engine) fn read_held(node: &HeldSnapshotNode) -> Result<Self> {
        let manifest = node.manifest();
        if node.binding().reference() != manifest.reference() {
            return Err(Failure::PublicationMismatch.into());
        }
        let mut leaves = BTreeMap::new();
        for (version, expected) in manifest.files() {
            let file = node.open_payload(*version).map_err(|e| match e {
                SnapshotStoreError::Disk(e) => e,
                SnapshotStoreError::Io(e) => e.into(),
                SnapshotStoreError::Payload(platform_io::payload::Error::Io(e)) => e.into(),
                SnapshotStoreError::Payload(e) => Error::Payload(e),
                _ => Failure::ImageMismatch.into(),
            })?;
            let metadata = file.metadata()?;
            let entity = crate::working_tree::records::local_entity(
                platform_io::fs_safety::linux::FileIdentity::from_metadata(&metadata),
            );
            if metadata.len() != expected.size || entity.inode() == 0 {
                return Err(Failure::ImageMismatch.into());
            }
            file.check_running()?;
            leaves.insert(*version, BodyLeaf {
                logical: *expected,
                operation: manifest.operation_id(),
                name: layout::file(&manifest.payloads()[version]),
                physical: OriginalLeaf::Present { entity },
            });
        }
        Self::from_owner(manifest, leaves)
    }

    /// An independently authenticated owner supplies the complete native
    /// manifest and each physically observed/frozen version exactly once.
    /// This constructs bounded pages only; writing or publishing is separate.
    pub(in crate::engine) fn from_owner(
        manifest: &AdmittedSnapshotManifest,
        leaves: BTreeMap<VersionId, BodyLeaf>,
    ) -> Result<Self> {
        if manifest.files().len() != leaves.len() {
            return Err(Failure::ImageMismatch.into());
        }
        let mut entities = BTreeSet::new();
        for (version, file) in manifest.files() {
            let leaf = leaves.get(version).ok_or(Failure::ImageMismatch)?;
            let payload = manifest.payloads().get(version).ok_or(Failure::ImageMismatch)?;
            let entity = match &leaf.physical {
                OriginalLeaf::Present { entity } => entity,
                OriginalLeaf::ReservedMove { entity, source_parent, .. } => {
                    if source_parent.inode() == 0 { return Err(Failure::ImageMismatch.into()); }
                    entity
                }
            };
            if leaf.logical != *file || leaf.name != layout::file(payload)
                || entity.inode() == 0 || !entities.insert(*entity)
            {
                return Err(Failure::ImageMismatch.into());
            }
        }
        let reference = manifest.reference();
        let mut rows = OrdinalBuilder::new(MAX_ENTRIES);
        for entry in manifest.entries() {
            let SnapshotEntryContent::File { material, .. } = entry.content else {
                // Empty directories stay in the admitted original native
                // manifest. They are not bodies, absent FILEs or retirements.
                continue;
            };
            let leaf = leaves.get(&material.version_id).ok_or(Failure::ImageMismatch)?;
            if leaf.logical != material { return Err(Failure::ImageMismatch.into()); }
            let name = leaf.name.as_os_str().to_str().ok_or(Failure::ImageMismatch)?;
            rows.push(&Body::DirectorySnapshot {
                source: EntrySource { snapshot: reference, entry },
                relative_name: name,
                bytes: material.size,
                content_hash: material.content_hash,
            })?;
        }
        Ok(Self { reference, operation: manifest.operation_id(), pages: rows.finish()?, leaves })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/scope_node_bodies.rs"]
mod tests;
