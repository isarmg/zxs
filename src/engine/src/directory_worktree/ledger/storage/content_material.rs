//! Read-only original ledger and two canonical successor encodings for the
//! physically admitted worktree. No append, repair, truncate or mutable ledger
//! is returned, including when FINALIZE left the tail part-installed.
use super::*;
use crate::snapshot_worktree::ScopeSource;

pub(crate) struct ScopedFrames {
    directory: DiskRoot,
    before: Checkpoint,
    before_bytes: usize,
    tail: Checkpoint,
    future: [(Checkpoint, Vec<u8>); 2],
}

impl ScopedFrames {
    pub(crate) fn observe(source: &ScopeSource<'_>) -> Result<Self, LedgerError> {
        let plan = source.plan();
        let value = source.result();
        let node = source.original();
        if node.binding() != plan.source_binding || node.manifest().reference() != plan.ledger.source() {
            return Err(LedgerError::Binding);
        }
        let directory = open_bound_directory(node.worktrees_directory(), plan.ledger)?;
        let events = plan.events(&value.rows);
        let (first, committing) = event_frame(plan.before, &events[0])?;
        let (second, completed) = event_frame(committing, &events[1])?;
        if [committing, completed] != [value.committing, value.completed] { return Err(LedgerError::Chain); }
        let (suffix, extra) = super::historical_mounts::physical_suffix(&directory, plan, completed, source.mount_history())?;
        let tail = if suffix == completed {
            super::published::content_tail(&directory, plan.before, committing, completed)?
        } else {
            verify_inventory(&directory, suffix)?;
            completed
        };
        if (!source.can_have_moved() && tail != plan.before) || (source.must_be_installed() && tail != completed) {
            return Err(LedgerError::Chain);
        }
        let mut state = None;
        let mut previous = None;
        let mut before_bytes = 0;
        for i in 0..=plan.before.sequence.get() {
            let (frame, checkpoint, _, bytes) = read(&directory, Sequence::new(i), previous)?;
            before_bytes = bounded_total(before_bytes, bytes.len())?;
            match frame.body {
                Body::Genesis(genesis) if i == 0 && checkpoint == plan.ledger.initial_checkpoint()
                    && genesis.binding == plan.instance && genesis.binding.worktree() == plan.ledger.worktree()
                    && genesis.binding.source() == plan.ledger.source()
                    && genesis.source.repository == plan.source.repository_id && genesis.source.node == plan.source.node_id
                    && genesis.source.manifest == plan.ledger.source().manifest => state = Some(State::new(&genesis)),
                Body::Event(event) => state.as_mut().ok_or(LedgerError::Chain)?.apply(node.manifest(), &event, checkpoint,
                    &mut |_, _| Err(LedgerError::FileResults), &mut |_, _| Err(LedgerError::FileResults))?,
                _ => return Err(LedgerError::Binding),
            }
            previous = Some(checkpoint.commitment);
        }
        if previous != Some(plan.before.commitment) { return Err(LedgerError::Chain); }
        let mut state = state.ok_or(LedgerError::Chain)?;
        state.require_publication_scope()?;
        if state.worktree.phase() != (Phase::Active { saved: None }) || state.worktree.revision() != plan.request.expected
            || !state.file_results.is_empty() || !state.edits.iter().eq(plan.rows.iter().map(|r| &r.edit))
        { return Err(LedgerError::FileResults); }
        bounded_total(bounded_total(bounded_total(before_bytes, first.len())?, second.len())?, extra)?;
        let mut verify = |p: &Publication, files: &[FileResult]| {
            if *p != plan.ledger_publication() || !files.iter().eq(value.rows.iter().map(|r| &r.file)) {
                return Err(LedgerError::FileResults);
            }
            Ok(())
        };
        for (event, checkpoint) in events.iter().zip([committing, completed]) {
            state.apply(node.manifest(), event, checkpoint, &mut verify, &mut |_, _| Err(LedgerError::FileResults))?;
        }
        let observed = Self { directory, before: plan.before, before_bytes, tail,
            future: [(committing, first), (completed, second)] };
        observed.future(0)?; observed.future(1)?;
        Ok(observed)
    }

    pub(crate) fn before_bytes(&self) -> usize { self.before_bytes }
    pub(crate) fn original(&self, sequence: Sequence, previous: Option<MaterialCommitment>)
        -> Result<(Checkpoint, LocalEntityIdentity, Vec<u8>), LedgerError> {
        if sequence > self.before.sequence { return Err(LedgerError::Chain); }
        let (_, checkpoint, entity, bytes) = read(&self.directory, sequence, previous)?;
        Ok((checkpoint, entity, bytes))
    }
    /// Never return an invented inode for a prospective frame. Conversely, an
    /// installed frame has to equal the original predicted bytes byte-for-byte.
    pub(crate) fn future(&self, ordinal: usize) -> Result<(Checkpoint, &[u8], Option<LocalEntityIdentity>), LedgerError> {
        let (checkpoint, expected) = self.future.get(ordinal).ok_or(LedgerError::Chain)?;
        let name = layout::frame(checkpoint.sequence);
        if checkpoint.sequence > self.tail.sequence {
            if self.directory.open_optional_read_entry(&name).map_err(LedgerError::Disk)?.is_some() { return Err(LedgerError::Chain); }
            return Ok((*checkpoint, expected, None));
        }
        let previous = if ordinal == 0 { self.before } else { self.future[0].0 };
        let (_, actual, entity, seen) = read(&self.directory, checkpoint.sequence, Some(previous.commitment))?;
        if actual != *checkpoint || seen != *expected { return Err(LedgerError::Chain); }
        Ok((*checkpoint, expected, Some(entity)))
    }
}

fn read(directory: &DiskRoot, sequence: Sequence, previous: Option<MaterialCommitment>)
    -> Result<(Frame, Checkpoint, LocalEntityIdentity, Vec<u8>), LedgerError> {
    use std::io::Read;
    let name = layout::frame(sequence);
    let mut file = directory.open_read_entry(&name).map_err(LedgerError::Disk)?;
    let entity = local_entity(platform_io::fs_safety::linux::FileIdentity::from_metadata(
        &file.metadata().map_err(LedgerError::Io)?));
    let mut bytes = Vec::new();
    (&mut file).take(MAX_FRAME_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(LedgerError::Io)?;
    if bytes.len() > MAX_FRAME_BYTES { return Err(LedgerError::Budget); }
    let frame: Frame = storage_contract::decode_strict(&bytes).map_err(LedgerError::Codec)?;
    if frame.previous != previous || frame.sequence != sequence || frame.bytes()? != bytes { return Err(LedgerError::Chain); }
    let checkpoint = frame.checkpoint(&bytes);
    Ok((frame, checkpoint, entity, bytes))
}
