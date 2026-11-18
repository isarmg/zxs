//! A cold digest and a freshly opened path are not interchangeable. Pin the
//! original manifest and every verified physical payload until the original
//! decision releases its scope. This read-only view contains no writer.
use super::*;
use crate::file_stamp::FileStamp;
use std::{collections::BTreeMap, os::unix::fs::MetadataExt};
use storage_contract::working_tree::SnapshotNodeId;

struct Pin { file: ReadFile, stamp: FileStamp, entity: LocalEntityIdentity }

impl Pin {
    fn new(file: ReadFile) -> crate::Result<Self> {
        let m = file.metadata()?;
        if !m.is_file() || m.nlink() != 1 || m.ino() == 0 { return Err(rejected()); }
        Ok(Self { file, stamp: FileStamp::from_metadata(&m), entity: local_entity(FileIdentity::from_metadata(&m)) })
    }
    fn require(&self, parent: &DiskRoot, name: &EntryName) -> crate::Result<()> {
        self.file.check_running()?;
        let named = parent.open_read_entry(name)?;
        if FileStamp::from_metadata(&named.metadata()?) != self.stamp
            || FileStamp::from_metadata(&self.file.metadata()?) != self.stamp { return Err(rejected()); }
        named.check_running()
    }
}
fn rejected() -> crate::Error { crate::engine::scope_closure::ScopeClosureError::ImageMismatch.into() }
fn dir_error(e: DirectoryIoError) -> crate::Error {
    match e { DirectoryIoError::Gate(e) => e, DirectoryIoError::Filesystem(e) => e.into(),
        DirectoryIoError::Io(e) => e.into(), _ => rejected() }
}
fn store_error(error: SnapshotStoreError) -> crate::Error {
    match error {
        SnapshotStoreError::Disk(e) => e, SnapshotStoreError::Io(e) => e.into(),
        SnapshotStoreError::Payload(platform_io::payload::Error::Io(e)) => e.into(),
        SnapshotStoreError::Payload(e) => crate::Error::Payload(e),
        SnapshotStoreError::Directory(e) => dir_error(e), _ => rejected(),
    }
}

impl SnapshotNodeBinding {
    pub(crate) fn repository_entity(&self) -> LocalEntityIdentity { self.repository_entity }
}

/// Inputs supplied here must already be the authenticated scope's exact
/// binding and actual ordinary host. Directory listing is only a negative
/// residual check after the original index has enumerated all obligations.
pub(crate) struct ScopedNodeFiles {
    node: HeldSnapshotNode, manifest: Pin, bodies: BTreeMap<VersionId, Pin>,
    _permit: crate::engine::PayloadProofReservation,
}
impl ScopedNodeFiles {
    pub(crate) fn open(host: &DiskRoot, binding: SnapshotNodeBinding,
                       original: &AdmittedSnapshotManifest) -> crate::Result<Self> {
        if original.reference() != binding.reference() { return Err(rejected()); }
        let n = original.files().len().checked_add(4).ok_or_else(rejected)?;
        let permit = crate::engine::payload_proof_budget().reserve(n)
            .ok_or(crate::engine::scope_closure::ScopeClosureError::Budget)?;
        let repo = host.private_entry_matching(&crate::persist::layout::SNAPSHOT_REPOSITORY,
            file_identity(binding.repository_entity)).map_err(dir_error)?;
        let node = HeldSnapshotNode::reopen_inventory(&repo, binding).map_err(store_error)?;
        if original.bytes() != node.manifest().bytes() { return Err(rejected()); }
        let mut manifest = Pin::new(node.node.open_read_entry(&layout::MANIFEST)?)?;
        if manifest.stamp.size != original.bytes().len() as u64 { return Err(rejected()); }
        let mut bytes = vec![0; original.bytes().len()];
        manifest.file.read_exact(&mut bytes)?;
        if bytes != original.bytes() { return Err(rejected()); }
        manifest.require(&node.node, &layout::MANIFEST)?;
        let mut bodies = BTreeMap::new();
        for (id, expected) in original.files() {
            // Hash the actual retained FD under the stamp that will be checked
            // at acceptance. A previous successful read is not a pin.
            let name = layout::file(&original.payloads()[id]);
            let mut pin = Pin::new(node.files.open_read_entry(&name)?)?;
            pin.file.verify_full(platform_io::payload::Expectation {
                bytes: expected.size, blake3: Some(expected.content_hash.digest()),
            })?;
            pin.require(&node.files, &name)?;
            bodies.insert(*id, pin);
        }
        let value = Self { node, manifest, bodies, _permit: permit };
        value.require(host)?;
        Ok(value)
    }
    pub(crate) fn binding(&self) -> SnapshotNodeBinding { self.node.binding() }
    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest { self.node.manifest() }
    pub(crate) fn body_entity(&self, id: VersionId) -> crate::Result<LocalEntityIdentity> {
        self.bodies.get(&id).map(|p| p.entity).ok_or_else(rejected)
    }
    pub(crate) fn require(&self, host: &DiskRoot) -> crate::Result<()> {
        let b = self.node.binding;
        let repo = host.private_entry_matching(&crate::persist::layout::SNAPSHOT_REPOSITORY,
            file_identity(b.repository_entity)).map_err(dir_error)?;
        repo.private_entry_matching(&layout::node(b.reference.node), file_identity(b.node_entity)).map_err(dir_error)?;
        self.node.node.private_entry_matching(&layout::FILES, file_identity(b.files_entity)).map_err(dir_error)?;
        self.node.node.private_entry_matching(&layout::WORKTREES, file_identity(b.worktrees_entity)).map_err(dir_error)?;
        self.node.verify_inventory().map_err(store_error)?;
        self.manifest.require(&self.node.node, &layout::MANIFEST)?;
        for (id, pin) in &self.bodies {
            pin.require(&self.node.files, &layout::file(&self.node.manifest.payloads()[id]))?;
        }
        Ok(())
    }
    pub(crate) fn require_no_instances(&self) -> crate::Result<()> {
        if self.node.worktrees.root_entries().map_err(dir_error)?.next().transpose()?.is_some() { return Err(rejected()); }
        Ok(())
    }
    pub(crate) fn require_repository_names(host: &DiskRoot, entity: LocalEntityIdentity,
                                          nodes: impl Iterator<Item = SnapshotNodeId>) -> crate::Result<()> {
        let repo = host.private_entry_matching(&crate::persist::layout::SNAPSHOT_REPOSITORY, file_identity(entity)).map_err(dir_error)?;
        let mut names: BTreeSet<_> = nodes.map(layout::node).collect();
        for name in repo.root_entries().map_err(dir_error)? {
            if !names.remove(&name?) { return Err(rejected()); }
        }
        if !names.is_empty() { return Err(rejected()); }
        Ok(())
    }
}
