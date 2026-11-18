//! No pathname, latest frame, or clean flag from the request admits these
//! transitions. The exact prior directory publication verifies every FILE row.
use super::*;
use crate::directory_worktree::lifecycle::DeletePlan;
use crate::persist::snapshot_worktree::{content_batch, mount_change};
use storage_contract::snapshot_worktree::mount_change::MountAction;

pub(crate) struct MountFrame {
    pub(crate) after: Checkpoint,
    pub(crate) bytes: Vec<u8>,
    pub(crate) observed: Option<LocalEntityIdentity>,
}

fn verify_files(
    content: &content_batch::Reservation, published: &content_batch::Publication,
    p: &Publication, files: &[FileResult],
) -> Result<(), LedgerError> {
    if *p != content.ledger_publication() || !published.rows.iter().map(|r| &r.file).eq(files.iter()) {
        return Err(LedgerError::FileResults);
    }
    Ok(())
}

impl HeldWorktreeLedger<'_> {
    fn require_clean_mount(
        &self, plan: &mount_change::Reservation, content: &content_batch::Reservation,
        published: &content_batch::Publication, installed: bool,
    ) -> Result<(), LedgerError> {
        self.state.require_publication_scope()?;
        if self.instance != plan.instance || self.binding != plan.ledger
            || self.worktree().revision() != plan.request.expected || !self.added_files().is_empty()
            || !self.edits().iter().eq(content.rows.iter().map(|r| &r.edit))
            || !self.file_results().iter().eq(published.rows.iter().map(|r| &r.file))
        {
            return Err(LedgerError::FileResults);
        }
        let p = content.ledger_publication();
        let active = Phase::Active { saved: Some(p) };
        let phase = self.worktree().phase();
        let mount = self.worktree().mount();
        let operation = plan.request.operation_id;
        match (&plan.request.action, &plan.resolved, installed) {
            (MountAction::Hide { .. }, mount_change::Resolved::Hide {}, false)
                if phase == active && *mount == plan.prior_mount => {
                let mut copy = self.worktree().clone();
                if copy.prepare_delete(operation).map_err(LedgerError::Lifecycle)? != DeletePlan::Retain {
                    return Err(LedgerError::FileResults);
                }
            }
            (MountAction::Hide { .. }, mount_change::Resolved::Hide {}, true)
                if phase == (Phase::Deleted { publication: p, deletion: operation })
                    && *mount == plan.prior_mount => {}
            (MountAction::Restore { deletion, .. }, mount_change::Resolved::Restore { .. }, false)
                if phase == (Phase::Deleted { publication: p, deletion: *deletion })
                    && *mount == plan.prior_mount => {}
            (MountAction::Restore { .. }, mount_change::Resolved::Restore { mount: result }, true)
                if phase == active && mount == result => {}
            _ => return Err(LedgerError::Binding),
        }
        Ok(())
    }
}

fn read_frame(
    source: &HeldSnapshotNode, plan: &mount_change::Reservation,
    content: &content_batch::Reservation, published: &content_batch::Publication,
    allow_after: bool, history: Option<&crate::engine::NativeMountHistory>,
) -> Result<MountFrame, LedgerError> {
    if source.binding() != content.source_binding || plan.source != content.source
        || plan.committed != content.handle() || plan.instance != content.instance
        || plan.ledger != content.ledger || plan.content_reserve != content.request.operation_id
        || published.reserve != plan.content_reserve
    {
        return Err(LedgerError::Binding);
    }
    let dir = open_bound_directory(source.worktrees_directory(), plan.ledger)?;
    let event = plan.event();
    let (bytes, after) = event_frame(plan.before, &event)?;
    let name = layout::frame(after.sequence);
    let file = dir.open_optional_read_entry(&name).map_err(LedgerError::Disk)?;
    let observed = match file {
        None => None,
        Some(mut file) if allow_after => {
            file.verify_full(Expectation { bytes: bytes.len() as u64, blake3: Some(blake3::hash(&bytes)) })
                .map_err(LedgerError::Payload)?;
            Some(local_entity(platform_io::fs_safety::linux::FileIdentity::from_metadata(
                &file.metadata().map_err(LedgerError::Io)?,
            )))
        }
        Some(_) => return Err(LedgerError::Chain),
    };
    let through = if observed.is_some() { after } else { plan.before };
    let suffix = super::historical_mounts::physical_suffix(&dir, content, after, history)?;
    if suffix.0 != after && observed.is_none() { return Err(LedgerError::Chain); }
    let inventory = if observed.is_none() { (through,0) } else { suffix };
    let ledger = HeldWorktreeLedger::mount_prefix(source, plan.ledger, through, inventory,
        |p, files| verify_files(content, published, p, files))?;
    ledger.require_clean_mount(plan, content, published, observed.is_some())?;
    if observed.is_none() {
        bounded_total(ledger.bytes, bytes.len())?;
        let mut logical = ledger.state.clone();
        logical.apply(source.manifest(), &event, after,
            &mut |p, files| verify_files(content, published, p, files),
            &mut |_, _| Err(LedgerError::FileResults))?;
    }
    Ok(MountFrame { after, bytes, observed })
}

pub(crate) fn observe_mount_frame(
    source: &HeldSnapshotNode, plan: &mount_change::Reservation,
    content: &content_batch::Reservation, published: &content_batch::Publication,
    allow_after: bool, history: Option<&crate::engine::NativeMountHistory>,
) -> Result<MountFrame, LedgerError> {
    read_frame(source, plan, content, published, allow_after, history)
}

pub(crate) fn install_mount_frame(
    proof: &crate::engine::FinalizedOriginalContent<'_>,
    source: &HeldSnapshotNode, plan: &mount_change::Reservation, result: mount_change::Publication,
    content: &content_batch::Reservation, published: &content_batch::Publication,
) -> Result<(), LedgerError> {
    proof.require_mount(plan, result).map_err(LedgerError::Disk)?;
    let observed = read_frame(source, plan, content, published, true, None)?;
    if result.checkpoint != observed.after { return Err(LedgerError::Chain); }
    let directory = open_bound_directory(source.worktrees_directory(), plan.ledger)?;
    let _activity = directory.retain_io_activity().map_err(LedgerError::Disk)?;
    if observed.observed.is_none() {
        let mut ledger = HeldWorktreeLedger::reopen(source, plan.ledger, plan.before,
            |p, files| verify_files(content, published, p, files))?;
        ledger.require_clean_mount(plan, content, published, false)?;
        if ledger.append(plan.event(), |p, files| verify_files(content, published, p, files))?
            != result.checkpoint
        {
            return Err(LedgerError::Chain);
        }
    }
    super::published::sync_frame(&directory, result.checkpoint, &observed.bytes)?;
    source.worktrees_directory().sync_private_entry_matching(&layout::ledger(plan.ledger.worktree()),
        file_identity(plan.ledger.ledger_entity)).map_err(|source| LedgerError::Persisted {
            intended: result.checkpoint, source,
        })?;
    if read_frame(source, plan, content, published, true, None)?.observed.is_none() {
        return Err(LedgerError::Chain);
    }
    Ok(())
}
