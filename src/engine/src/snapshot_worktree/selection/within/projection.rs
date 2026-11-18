//! The normal full directory manifest for a batch that adds selected mounts.
//! Nothing here writes a node, publishes FILEs, or treats an existing snapshot
//! path as replaceable merely because a new mount uses the same spelling.
use super::HeldMountedFiles;
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent,
    SnapshotFile, SnapshotFileSource, SnapshotStoreError, MAX_ENTRIES};
use crate::{Engine,ReadFile};
use name_policy::PortablePath;
use std::collections::{BTreeMap,btree_map::Entry};
use storage_contract::{OperationId,VersionId,working_tree::SnapshotNodeId};

pub(crate) struct ProjectedMountedFiles {
    held: HeldMountedFiles,
    manifest: AdmittedSnapshotManifest,
    // The native snapshot format stores one independent FULL per immutable
    // VersionId. All path/branch/node/issuer occurrences are retained above.
    selected_body: BTreeMap<VersionId,usize>,
    bytes: u64,
}
impl HeldMountedFiles {
    pub(crate) fn project_new_mounts(self, engine:&Engine, node:SnapshotNodeId, operation:OperationId)
        -> Result<ProjectedMountedFiles> {
        self.require_current(engine)?;
        if node == self.outer.node_id || engine.state().operations.contains_key(&operation) {
            return Err(RegistrationError::Operation(operation));
        }
        let mut by_path:BTreeMap<_,_> = self.node.manifest().entries().iter().map(|entry|
            (entry.path.key().to_owned(),entry.clone())).collect();
        let limit = MAX_ENTRIES.min(engine.config().max_entries);
        if by_path.len() > limit { return Err(RegistrationError::Manifest(crate::working_tree::snapshot::SnapshotManifestError::Budget)); }
        let mut selected_body = BTreeMap::new();
        for (index,(path,held)) in self.rows.iter().enumerate() {
            let row = held.facts();
            let root = row.mount.name.key();
            // A prior captured mount will need its issuing lineage to update;
            // never merge a reused mount into an unrelated historical subtree.
            if self.node.manifest().entries().binary_search_by(|e|e.path.key().cmp(&root)).is_ok() {
                return Err(RegistrationError::Mount);
            }
            let mut parent = path.parent();
            while let Some(dir) = parent {
                parent = dir.parent();
                insert_parent(&mut by_path,dir,limit)?;
            }
            let material = SnapshotFile {file_id:row.selected.file,version_id:row.head.version,
                size:row.head.size,content_hash:row.head.hash};
            if by_path.len() >= limit || by_path.contains_key(path.key()) { return Err(RegistrationError::Mount); }
            by_path.insert(path.key().to_owned(),SnapshotEntry { path:path.clone(),content:SnapshotEntryContent::File {
                material,source:SnapshotFileSource::Container {branch:row.selected.branch,node:row.head.node},
            }});
            selected_body.entry(material.version_id).or_insert(index);
        }
        let manifest = AdmittedSnapshotManifest::prepare(self.outer.repository_id,node,operation,by_path.into_values().collect())
            .map_err(RegistrationError::Manifest)?;
        let overflow = || RegistrationError::Publication(crate::engine::fresh_layout::FreshLayoutError::PhysicalBytesOverflow.into());
        let header = u64::try_from(manifest.encoded_len()).map_err(|_|overflow())?;
        let bytes = manifest.files().values().try_fold(header,|sum,row|sum.checked_add(row.size).ok_or_else(overflow))?;
        Ok(ProjectedMountedFiles {held:self,manifest,selected_body,bytes})
    }
}
fn insert_parent(rows:&mut BTreeMap<String,SnapshotEntry>,path:PortablePath,limit:usize) -> Result<()> {
    let full = rows.len() >= limit;
    match rows.entry(path.key().to_owned()) {
        Entry::Occupied(old) if old.get().path == path && matches!(old.get().content,SnapshotEntryContent::Directory {}) => Ok(()),
        Entry::Vacant(free) if !full => {free.insert(SnapshotEntry {path,content:SnapshotEntryContent::Directory {}}); Ok(())},
        _ => Err(RegistrationError::Mount),
    }
}
impl ProjectedMountedFiles {
    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest { &self.manifest }
    pub(crate) fn independent_bytes(&self) -> u64 { self.bytes }
    pub(crate) fn source(&self) -> &HeldMountedFiles { &self.held }
    pub(crate) fn open_body(&self, expected:&SnapshotFile) -> std::result::Result<ReadFile,SnapshotStoreError> {
        if self.manifest.files().get(&expected.version_id) != Some(expected) { return Err(SnapshotStoreError::Version); }
        if let Some(index) = self.selected_body.get(&expected.version_id) {
            // If an old outer row and new mounted row select the same version,
            // copy from the just-verified actual inner FILE, not a stale alias.
            let source = &self.held.rows[*index].1;
            let row=source.facts();
            if row.selected.file != expected.file_id || row.head.size != expected.size || row.head.hash != expected.content_hash {
                return Err(SnapshotStoreError::Version);
            }
            source.body.open_original().map_err(SnapshotStoreError::Disk)
        } else {
            if self.held.node.manifest().files().get(&expected.version_id) != Some(expected) { return Err(SnapshotStoreError::Version); }
            self.held.node.open_payload(expected.version_id)
        }
    }
}
