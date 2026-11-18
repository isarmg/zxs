//! Descriptive .zxs source for reopening/rebuilding the *original* compound
//! Worktree copy. This is neither the physical FILE reader nor proof that a
//! particular mapping participated in that snapshot.
use super::{Failure, Result, occurrence};
use crate::persist::snapshot_worktree::HostEvidence;
use crate::snapshot_worktree::ReceiptIndex;
use crate::working_tree::snapshot::{
    HeldSnapshotNode, SnapshotEntryContent, SnapshotFile, SnapshotFileSource, SnapshotNodeBinding,
};
use crate::State;
use name_policy::PortablePath;
use storage_contract::snapshot_worktree::SnapshotHandle;

pub(crate) struct RecordedSnapshotFile<'a> {
    previous: &'a State,
    handle: SnapshotHandle,
    path: PortablePath,
    host: HostEvidence,
    binding: SnapshotNodeBinding,
    material: SnapshotFile,
    origin: SnapshotFileSource,
}

impl ReceiptIndex<'_> {
    /// A caller cannot choose the FILE, Version, node, root or bytes independently
    /// of the encoded original manifest at its exact committed handle and path.
    pub(crate) fn original_copy_file<'s>(
        &self, previous: &'s State, handle: SnapshotHandle, path: &PortablePath,
    ) -> Result<RecordedSnapshotFile<'s>> {
        self.require_source(previous)?;
        let registered = self.node(handle)?;
        let selected = occurrence(registered.manifest().entries(), path)?;
        let SnapshotEntryContent::File {material, source} = selected.content else {
            return Err(Failure::Source);
        };
        Ok(RecordedSnapshotFile {
            previous, handle, path: path.clone(), host: registered.host().clone(),
            binding: registered.binding(), material, origin: source,
        })
    }
}

impl RecordedSnapshotFile<'_> {
    pub(crate) fn host(&self) -> &HostEvidence { &self.host }
    pub(crate) fn material(&self) -> SnapshotFile { self.material }
    pub(crate) fn origin(&self) -> SnapshotFileSource { self.origin }

    pub(crate) fn require_at(
        &self, previous: &State, handle: SnapshotHandle, path: &PortablePath,
    ) -> Result<()> {
        if !std::ptr::eq(self.previous, previous) || self.handle != handle || &self.path != path {
            return Err(Failure::Source);
        }
        Ok(())
    }

    pub(super) fn require_node(&self, node: &HeldSnapshotNode) -> Result<()> {
        let entry = occurrence(node.manifest().entries(), &self.path)?;
        if node.binding() != self.binding || entry.content != (SnapshotEntryContent::File {
            material: self.material, source: self.origin,
        }) {
            return Err(Failure::Source);
        }
        Ok(())
    }

    /// No current FILE read or unretire. Only identical original container
    /// Version metadata supplies its canonical chunk commitment. If it has
    /// ceased to exist, was snapshot-only, or used a different checksum chunk
    /// geometry, the compound owner must supply measured original .zxs bytes.
    pub(crate) fn original_checksums(&self, chunk_bytes: u32) -> Result<&str> {
        if !matches!(self.origin, SnapshotFileSource::Container{..}) { return Err(Failure::Source); }
        let body = self.previous.versions.get(&self.material.version_id.to_string())
            .ok_or(Failure::Source)?;
        if body.version_id != self.material.version_id.to_string()
            || body.file_id != self.material.file_id.to_string()
            || body.logical_size != self.material.size || body.content_hash != self.material.content_hash
            || body.checksum_chunk_bytes != chunk_bytes
        {
            return Err(Failure::Source);
        }
        Ok(&body.checksum_manifest_hash)
    }
}
