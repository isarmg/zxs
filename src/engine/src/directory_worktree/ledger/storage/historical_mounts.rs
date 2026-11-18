//! Historical reads validate an original checkpoint without accepting a newer
//! ledger as that operation's state. Later bytes require native fixed-member
//! decisions in order, the same concrete ledger, and that original FILE batch.
use super::*;
use crate::engine::NativeMountHistory;
use crate::persist::snapshot_worktree::content_batch;

pub(super) fn physical_suffix(
    dir: &DiskRoot, content: &content_batch::Reservation, checkpoint: Checkpoint,
    history: Option<&NativeMountHistory>,
) -> Result<(Checkpoint, usize), LedgerError> {
    let mut through = checkpoint;
    let mut bytes = 0;
    let Some(history) = history else { return Ok((through, bytes)); };
    for row in history.rows(content.request.target.worktree_id).iter()
        .filter(|row| row.result.checkpoint.sequence > checkpoint.sequence)
    {
        let plan = &row.plan;
        if plan.before != through || plan.request.target != content.request.target
            || plan.host != content.host || plan.root != content.root || plan.source != content.source
            || plan.committed != content.handle() || plan.content_reserve != content.request.operation_id
            || plan.instance != content.instance || plan.ledger != content.ledger
        { return Err(LedgerError::Binding); }
        let (encoded, after) = event_frame(through, &plan.event())?;
        if after != row.result.checkpoint || after.sequence.get() >= MAX_FRAMES { return Err(LedgerError::Chain); }
        let Some(file) = dir.open_optional_read_entry(&layout::frame(after.sequence)).map_err(LedgerError::Disk)? else {
            if row.published { return Err(LedgerError::Chain); }
            // The history owner permits at most one terminal, fully FINALIZED,
            // un-Published successor. Its frame may not have been installed.
            continue;
        };
        file.verify_full(Expectation { bytes: encoded.len() as u64, blake3: Some(blake3::hash(&encoded)) })
            .map_err(LedgerError::Payload)?;
        bytes = bounded_total(bytes, encoded.len())?;
        through = after;
    }
    Ok((through, bytes))
}

impl<'a> HeldWorktreeLedger<'a> {
    // Private prefix loan. Never append at this historical position.
    pub(super) fn mount_prefix(
        source: &'a HeldSnapshotNode, binding: Binding, expected: Checkpoint,
        suffix: (Checkpoint,usize),
        verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
    ) -> Result<Self, LedgerError> {
        if suffix.0.sequence < expected.sequence { return Err(LedgerError::Chain); }
        match Self::recover_frames(source,binding,expected,verify,
            |_,_| Err(LedgerError::FileResults),None,Some(suffix))? {
            RecoveredLedger::Ready(mut value) => { value.append = AppendState::Uncertain; Ok(value) }
            RecoveredLedger::Publishing(_) => Err(LedgerError::PublicationPending),
        }
    }

    pub(crate) fn require_content_checkpoint(
        source: &'a HeldSnapshotNode, plan: &content_batch::Reservation,
        result: &content_batch::Publication, history: Option<&NativeMountHistory>,
    ) -> Result<(), LedgerError> {
        let dir = open_bound_directory(source.worktrees_directory(), plan.ledger)?;
        let suffix = physical_suffix(&dir, plan, result.completed, history)?;
        let value = Self::mount_prefix(source, plan.ledger, result.completed, suffix, |p,files| {
            if *p != plan.ledger_publication() || !result.rows.iter().map(|r| &r.file).eq(files.iter()) {
                return Err(LedgerError::FileResults);
            }
            Ok(())
        })?;
        if value.instance() != plan.instance || value.worktree().revision() != plan.request.expected
            || value.file_results().iter().ne(result.rows.iter().map(|r| &r.file))
        { return Err(LedgerError::FileResults); }
        Ok(())
    }
}
