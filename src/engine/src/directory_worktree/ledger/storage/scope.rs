//! Read the original decided empty sparse instance and its named genesis
//! frame. An allocation, a staged directory or even a valid self-described
//! frame cannot enter this reader without the independently selected binding.
use super::*;
use crate::{ReadFile, file_stamp::FileStamp};
use std::{io::Read, os::unix::fs::MetadataExt};
use storage_contract::snapshot_worktree::CreateWorktree;

pub(crate) struct ScopedGenesis {
    binding: Binding, sparse: HeldSparseWorktree, directory: DiskRoot,
    frame: ReadFile, stamp: FileStamp, mount: Mount, _permit: crate::engine::PayloadProofReservation,
}
impl ScopedGenesis {
    pub(crate) fn open(source: &HeldSnapshotNode, expected: &CreateWorktree, binding: Binding,
                       checkpoint: Checkpoint) -> Result<Self, LedgerError> {
        if checkpoint != binding.initial_checkpoint() || checkpoint.sequence.get() != 0
            || source.binding().reference() != binding.source { return Err(LedgerError::Binding); }
        let permit = crate::engine::payload_proof_budget().reserve(3).ok_or(LedgerError::Budget)?;
        let original = HeldWorktreeLedger::reopen(source, binding, checkpoint, |_, _| Err(LedgerError::PublicationPending))?;
        let mount = original.worktree().mount().clone();
        let instance = original.instance();
        if original.worktree().phase() != (Phase::Active { saved: None }) || !original.edits().is_empty()
            || instance.operation() != expected.operation_id || original.worktree().source().branch != expected.source.branch_id
            || mount.parent != expected.parent_directory_id || mount.name.as_str() != expected.name.as_str()
            || instance.source() != source.manifest().reference() || instance.worktree() != binding.worktree {
            return Err(LedgerError::Binding);
        }
        let sparse = HeldSparseWorktree::reopen(source, instance).map_err(LedgerError::Instance)?;
        let directory = open_bound_directory(source.worktrees_directory(), binding)?;
        let mut file = directory.open_read_entry(&layout::frame(checkpoint.sequence)).map_err(LedgerError::Disk)?;
        let meta = file.metadata().map_err(LedgerError::Io)?;
        if !meta.is_file() || meta.nlink() != 1 || meta.len() > MAX_FRAME_BYTES as u64 || meta.ino() == 0 {
            return Err(LedgerError::Binding);
        }
        let stamp = FileStamp::from_metadata(&meta);
        let mut bytes = vec![0; meta.len() as usize];
        file.read_exact(&mut bytes).map_err(LedgerError::Io)?;
        let frame: Frame = storage_contract::decode_strict(&bytes).map_err(LedgerError::Codec)?;
        if frame.sequence != checkpoint.sequence || frame.previous.is_some() || frame.bytes()? != bytes
            || frame.checkpoint(&bytes) != checkpoint
            || !matches!(&frame.body, Body::Genesis(g) if g.binding == instance && g.source == original.worktree().source()
                && g.mount == mount) { return Err(LedgerError::Binding); }
        let result = Self { binding, sparse, directory, frame: file, stamp, mount, _permit: permit };
        result.require(source)?;
        Ok(result)
    }
    pub(crate) fn sparse(&self) -> SparseWorktreeBinding { self.sparse.binding() }
    pub(crate) fn entity(&self) -> Result<LocalEntityIdentity, LedgerError> {
        Ok(local_entity(self.sparse.directory().held_identity().map_err(LedgerError::Directory)?))
    }
    pub(crate) fn mount(&self) -> &Mount { &self.mount }
    pub(crate) fn require(&self, source: &HeldSnapshotNode) -> Result<(), LedgerError> {
        let reopened = HeldSparseWorktree::reopen(source, self.sparse()).map_err(LedgerError::Instance)?;
        let actual = open_bound_directory(source.worktrees_directory(), self.binding)?;
        if self.directory.held_identity().map_err(LedgerError::Directory)?
            != actual.held_identity().map_err(LedgerError::Directory)?
            || reopened.directory().held_identity().map_err(LedgerError::Directory)?
            != self.sparse.directory().held_identity().map_err(LedgerError::Directory)?
        { return Err(LedgerError::Binding); }
        verify_inventory(&actual, self.binding.initial_checkpoint())?;
        if reopened.directory().root_entries().map_err(LedgerError::Directory)?.next()
            .transpose().map_err(|e| LedgerError::Directory(crate::disk::DirectoryIoError::Filesystem(e)))?.is_some()
        { return Err(LedgerError::Binding); }
        let named = actual.open_read_entry(&layout::frame(self.binding.initial_checkpoint().sequence)).map_err(LedgerError::Disk)?;
        if FileStamp::from_metadata(&named.metadata().map_err(LedgerError::Io)?) != self.stamp
            || FileStamp::from_metadata(&self.frame.metadata().map_err(LedgerError::Io)?) != self.stamp {
            return Err(LedgerError::Binding);
        }
        self.frame.check_running().map_err(LedgerError::Disk)
    }
}
