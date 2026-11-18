//! Retained native Ledger frames and sparse payloads through one independently
//! decided Active checkpoint. All original edit leaves are retained, not just
//! the latest body per visible path. Never elect a ledger from its directory.
use super::*;
use crate::{ReadFile, file_stamp::FileStamp};
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::{collections::BTreeMap, io::Read, os::unix::fs::MetadataExt};
use storage_contract::snapshot_worktree::{CreateWorktree, WorktreeRevision};

pub(crate) struct ScopedEdit<'a> {
    pub(crate) operation: storage_contract::OperationId,
    pub(crate) expected: WorktreeRevision,
    pub(crate) before: Checkpoint, pub(crate) after: Checkpoint, pub(crate) record: &'a super::super::EditRecord,
}
struct Pin { file: ReadFile, stamp: FileStamp }
impl Pin {
    fn open(parent: &DiskRoot, name: &EntryName) -> Result<Self, LedgerError> {
        let file = parent.open_read_entry(name).map_err(LedgerError::Disk)?;
        let m = file.metadata().map_err(LedgerError::Io)?;
        if !m.is_file() || m.nlink() != 1 || m.ino() == 0 { return Err(LedgerError::Binding); }
        Ok(Self { file, stamp: FileStamp::from_metadata(&m) })
    }
    fn require(&self, parent: &DiskRoot, name: &EntryName) -> Result<(), LedgerError> {
        let named = parent.open_read_entry(name).map_err(LedgerError::Disk)?;
        if FileStamp::from_metadata(&named.metadata().map_err(LedgerError::Io)?) != self.stamp
            || FileStamp::from_metadata(&self.file.metadata().map_err(LedgerError::Io)?) != self.stamp {
            return Err(LedgerError::Binding);
        }
        self.file.check_running().map_err(LedgerError::Disk)
    }
}
pub(crate) struct ScopedActiveLedger {
    binding: Binding, sparse: HeldSparseWorktree, directory: DiskRoot, through: Checkpoint,
    frames: Vec<Pin>, leaves: BTreeMap<EntryName, Pin>, mount: Mount, _permit: crate::engine::PayloadProofReservation,
}
impl ScopedActiveLedger {
    pub(crate) fn open(source: &HeldSnapshotNode, original: &CreateWorktree, binding: Binding,
        initial: Checkpoint, edits: &[ScopedEdit<'_>]) -> Result<Self, LedgerError> {
        let through = edits.last().map_or(initial, |e| e.after);
        if initial != binding.initial_checkpoint() || initial.sequence.get() != 0 || through.sequence.get() != edits.len() as u64
            || through.sequence.get() >= MAX_FRAMES || source.binding().reference() != binding.source {
            return Err(LedgerError::Binding);
        }
        let n = edits.len().checked_mul(2).and_then(|n| n.checked_add(3)).ok_or(LedgerError::Budget)?;
        let permit = crate::engine::payload_proof_budget().reserve(n).ok_or(LedgerError::Budget)?;
        let native = HeldWorktreeLedger::reopen(source, binding, through, |_, _| Err(LedgerError::PublicationPending))?;
        let mount = native.worktree().mount().clone();
        let instance = native.instance();
        if native.worktree().phase() != (Phase::Active { saved: None })
            || instance.operation() != original.operation_id || native.worktree().source().branch != original.source.branch_id
            || mount.parent != original.parent_directory_id || mount.name.as_str() != original.name.as_str()
            || instance.source() != source.manifest().reference() || instance.worktree() != binding.worktree {
            return Err(LedgerError::Binding);
        }
        let sparse = HeldSparseWorktree::reopen(source, instance).map_err(LedgerError::Instance)?;
        let directory = open_bound_directory(source.worktrees_directory(), binding)?;
        let mut frames = Vec::with_capacity(edits.len() + 1);
        let mut leaves = BTreeMap::new();
        let mut total = 0usize;
        let mut previous = None;
        let mut entities = BTreeSet::new();
        for idx in 0..=edits.len() {
            let seq = Sequence::new(idx as u64);
            let name = layout::frame(seq);
            let mut pin = Pin::open(&directory, &name)?;
            let size = usize::try_from(pin.stamp.size).map_err(|_| LedgerError::Budget)?;
            if size > MAX_FRAME_BYTES { return Err(LedgerError::Budget); }
            total = bounded_total(total, size)?;
            let mut bytes = vec![0; size];
            pin.file.read_exact(&mut bytes).map_err(LedgerError::Io)?;
            let frame: Frame = storage_contract::decode_strict(&bytes).map_err(LedgerError::Codec)?;
            let at = frame.checkpoint(&bytes);
            if frame.sequence != seq || frame.previous != previous || frame.bytes()? != bytes {
                return Err(LedgerError::Chain);
            }
            match (idx.checked_sub(1).map(|i| &edits[i]), &frame.body) {
                (None, Body::Genesis(g)) if at == initial && g.binding == instance && g.mount == mount
                    && g.source == native.worktree().source() => {}
                (Some(e), Body::Event(Event::Edits { operation, expected, edits: actual })) if *operation == e.operation
                    && *expected == e.expected && actual.as_slice() == std::slice::from_ref(e.record)
                    && Some(e.before.commitment) == previous && e.after == at => {
                    let record = e.record;
                    let name = layout::edited_file(record.material.version_id, &record.path);
                    if &name != record.leaf.entry() || !entities.insert(record.entity) { return Err(LedgerError::Binding); }
                    let mut data = Pin::open(sparse.directory(), &name)?;
                    let metadata = data.file.metadata().map_err(LedgerError::Io)?;
                    if local_entity(FileIdentity::from_metadata(&metadata)) != record.entity
                        || metadata.len() != record.material.size { return Err(LedgerError::Binding); }
                    data.file.verify_full(Expectation { bytes: record.material.size,
                        blake3: Some(record.material.content_hash.digest()) }).map_err(LedgerError::Payload)?;
                    data.require(sparse.directory(), &name)?;
                    if leaves.insert(name, data).is_some() { return Err(LedgerError::Binding); }
                }
                _ => return Err(LedgerError::Binding),
            }
            pin.require(&directory, &name)?;
            previous = Some(at.commitment);
            frames.push(pin);
        }
        if previous != Some(through.commitment) { return Err(LedgerError::Chain); }
        let result = Self { binding, sparse, directory, through, frames, leaves, mount, _permit: permit };
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
        verify_inventory(&actual, self.through)?;
        for (i, pin) in self.frames.iter().enumerate() {
            pin.require(&actual, &layout::frame(Sequence::new(i as u64)))?;
        }
        let mut names: BTreeSet<_> = self.leaves.keys().cloned().collect();
        for name in reopened.directory().root_entries().map_err(LedgerError::Directory)? {
            let name = name.map_err(|e| LedgerError::Directory(crate::disk::DirectoryIoError::Filesystem(e)))?;
            if !names.remove(&name) { return Err(LedgerError::Binding); }
        }
        if !names.is_empty() { return Err(LedgerError::Binding); }
        for (name, pin) in &self.leaves { pin.require(reopened.directory(), name)?; }
        Ok(())
    }
}
