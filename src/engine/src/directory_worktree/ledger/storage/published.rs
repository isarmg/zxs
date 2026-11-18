//! Finish only the exact two frames selected by an independently published node.
//! Local inventory chooses between those two already determined checkpoints;
//! it cannot supply a third event, a new source or a different ledger entity.
use crate::directory_worktree::ledger::storage::{
    HeldWorktreeLedger, RecoveredLedger, event_frame, open_bound_directory,
};
use crate::directory_worktree::ledger::{AddedFileResult, Binding, Checkpoint, Event, LedgerError};
use crate::directory_worktree::lifecycle::Publication;
use crate::directory_worktree::projection::FileResult;
use crate::disk::DiskRoot;
use crate::persist::layout::worktree as layout;
use crate::snapshot_worktree::{PublishedAddedCommitTail, PublishedCommitTail};
use crate::working_tree::records::file_identity;
use crate::working_tree::snapshot::HeldSnapshotNode;
use platform_io::payload::Expectation;
use std::collections::BTreeSet;
use storage_contract::Sequence;

impl<'a> HeldWorktreeLedger<'a> {
    pub(crate) fn finish_published_commit(
        source: &'a HeldSnapshotNode,
        node: &HeldSnapshotNode,
        published: &PublishedCommitTail<'_>,
    ) -> Result<Self, LedgerError> {
        published.require_nodes(source, node)?;
        Self::finish_published_transitions(source, published.binding(), published.before(),
            published.transitions(),
            |publication, files| published.require_publication(publication, files),
            |_, _| Err(LedgerError::FileResults))
    }

    pub(crate) fn finish_published_added_commit(
        source: &'a HeldSnapshotNode,
        node: &HeldSnapshotNode,
        published: &PublishedAddedCommitTail<'_>,
    ) -> Result<Self, LedgerError> {
        published.require_nodes(source, node)?;
        Self::finish_published_transitions(source, published.binding(), published.before(),
            published.transitions(), |_, _| Err(LedgerError::FileResults),
            |publication, file| published.require_publication(publication, file))
    }

    // Only the two opaque, independently published tails above can enter here.
    fn finish_published_transitions(
        source: &'a HeldSnapshotNode,
        binding: Binding,
        before: Checkpoint,
        transitions: [Event; 2],
        mut verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
        mut verify_added: impl FnMut(&Publication, &AddedFileResult) -> Result<(), LedgerError>,
    ) -> Result<Self, LedgerError> {
        let [commit, completed] = transitions;
        let (commit_bytes, committing) = event_frame(before, &commit)?;
        let (completed_bytes, finished) = event_frame(committing, &completed)?;
        let directory = open_bound_directory(source.worktrees_directory(), binding)?;
        let _activity = directory.retain_io_activity().map_err(LedgerError::Disk)?;
        let checkpoint = selected_tail(&directory, committing, finished)?;
        // The normal reader replays every canonical frame from Genesis, checks
        // the entire hash chain and original instance, and verifies completion
        // against the same permanent FILE/node result. No prefix is skipped.
        let recovered = Self::recover_with_added(source, binding, checkpoint,
            &mut verify, &mut verify_added)?;
        let mut ledger = match recovered {
            RecoveredLedger::Ready(ledger) => ledger,
            RecoveredLedger::Publishing(pending) => pending.ledger,
        };
        if checkpoint == committing {
            ledger.append_with_added(completed, &mut verify, &mut verify_added)?;
        }
        // A matching but previously unacknowledged frame still owes its file
        // and parent durability tail. Never publish SYSTEM completion merely
        // because those bytes were readable after an interrupted append.
        sync_frame(&ledger.directory, committing, &commit_bytes)?;
        sync_frame(&ledger.directory, finished, &completed_bytes)?;
        source.worktrees_directory().sync_private_entry_matching(
            &layout::ledger(binding.worktree), file_identity(binding.ledger_entity),
        ).map_err(|source| LedgerError::Persisted { intended: finished, source })?;
        ledger.admit_read_view()?;
        Ok(ledger)
    }
}

fn selected_tail(directory: &DiskRoot, committing: Checkpoint, finished: Checkpoint)
    -> Result<Checkpoint, LedgerError>
{
    let mut required: BTreeSet<_> = (0..=committing.sequence.get())
        .map(|index| layout::frame(Sequence::new(index))).collect();
    let final_name = layout::frame(finished.sequence);
    let mut complete = false;
    for name in directory.root_entries().map_err(LedgerError::Directory)? {
        let name = name.map_err(LedgerError::Inventory)?;
        if name == final_name {
            if complete {
                return Err(LedgerError::Chain);
            }
            complete = true;
        } else if !required.remove(&name) {
            return Err(LedgerError::Chain);
        }
    }
    if !required.is_empty() {
        return Err(LedgerError::Chain);
    }
    Ok(if complete { finished } else { committing })
}

fn sync_frame(directory: &DiskRoot, checkpoint: Checkpoint, bytes: &[u8])
    -> Result<(), LedgerError>
{
    let mut file = directory.open_read_entry(&layout::frame(checkpoint.sequence))
        .map_err(LedgerError::Disk)?;
    file.verify_full(Expectation {
        bytes: bytes.len() as u64, blake3: Some(blake3::hash(bytes)),
    }).map_err(LedgerError::Payload)?;
    file.sync_all().map_err(LedgerError::Io)
}

impl<'a> HeldWorktreeLedger<'a> {
    pub(crate) fn finish_finalized_content(
        source: &'a HeldSnapshotNode, node: &HeldSnapshotNode,
        plan: &crate::persist::snapshot_worktree::content_batch::Reservation,
        value: &crate::persist::snapshot_worktree::content_batch::Publication,
        decision: &crate::engine::FinalizedOriginalContent<'_>,
        cut: &mut impl FnMut(&str) -> crate::Result<()>,
    ) -> Result<Self, LedgerError> {
        decision.require(plan, value).map_err(LedgerError::Disk)?;
        if source.binding() != plan.source_binding || node.binding() != value.prepared_node.binding
            || node.manifest().entries() != value.prepared_node.entries || node.manifest().operation_id() != plan.publish_operation
        { return Err(LedgerError::FileResults); }
        let [first, second] = plan.events(&value.rows);
        let (first_bytes, committing) = event_frame(plan.before, &first)?;
        let (second_bytes, completed) = event_frame(committing, &second)?;
        if committing != value.committing || completed != value.completed { return Err(LedgerError::Chain); }
        let dir = open_bound_directory(source.worktrees_directory(), plan.ledger)?;
        let _activity = dir.retain_io_activity().map_err(LedgerError::Disk)?;
        let checkpoint = content_tail(&dir, plan.before, committing, completed)?;
        let mut verify = |publication: &Publication, files: &[FileResult]| {
            if *publication != plan.ledger_publication() || !value.rows.iter().map(|r| &r.file).eq(files.iter()) {
                return Err(LedgerError::FileResults);
            }
            Ok(())
        };
        let mut ledger = match Self::recover_frames(source, plan.ledger, checkpoint, &mut verify,
            |_, _| Err(LedgerError::FileResults), Some(decision))? {
            RecoveredLedger::Ready(ledger) => ledger, RecoveredLedger::Publishing(pending) => pending.ledger,
        };
        if ledger.instance() != plan.instance || !plan.rows.iter().map(|r| &r.edit).eq(ledger.edits().iter()) {
            return Err(LedgerError::Binding);
        }
        if ledger.checkpoint() == plan.before && ledger.append(first, &mut verify)? != committing {
            return Err(LedgerError::Chain);
        }
        sync_frame(&ledger.directory, committing, &first_bytes)?;
        cut("after_worktree_content_committing").map_err(LedgerError::Disk)?;
        if ledger.checkpoint() == committing && ledger.append(second, &mut verify)? != completed {
            return Err(LedgerError::Chain);
        }
        if ledger.checkpoint() != completed { return Err(LedgerError::Chain); }
        sync_frame(&ledger.directory, completed, &second_bytes)?;
        source.worktrees_directory().sync_private_entry_matching(&layout::ledger(plan.ledger.worktree()),
            file_identity(plan.ledger.ledger_entity)).map_err(|source| LedgerError::Persisted { intended: completed, source })?;
        ledger.admit_read_view()?;
        cut("after_worktree_content_completed").map_err(LedgerError::Disk)?;
        Ok(ledger)
    }
}

fn content_tail(directory: &DiskRoot, before: Checkpoint, first: Checkpoint, second: Checkpoint)
    -> Result<Checkpoint, LedgerError>
{
    let mut required: BTreeSet<_> = (0..=before.sequence.get()).map(|i| layout::frame(Sequence::new(i))).collect();
    let one = layout::frame(first.sequence);
    let two = layout::frame(second.sequence);
    let mut has_one = false; let mut has_two = false;
    for name in directory.root_entries().map_err(LedgerError::Directory)? {
        let name = name.map_err(LedgerError::Inventory)?;
        if name == one && !has_one { has_one = true; }
        else if name == two && !has_two { has_two = true; }
        else if !required.remove(&name) { return Err(LedgerError::Chain); }
    }
    if !required.is_empty() || (has_two && !has_one) { return Err(LedgerError::Chain); }
    Ok(if has_two { second } else if has_one { first } else { before })
}
