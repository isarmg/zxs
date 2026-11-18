//! Ordinary publication cannot shadow independently reserved worktree mounts.
use crate::engine::PlanningContext;
use crate::snapshot_worktree::{ReceiptIndex, SnapshotSourceError};
use crate::{Error, State};

impl PlanningContext<'_> {
    /// The publication caller retains this exact source through preparation
    /// and decision. No disk names or candidate-authored receipts grant claims.
    pub(crate) fn require_worktree_mount_reservations(&self, next: &State) -> crate::Result<()> {
        if !self.state.operations.values().any(ReceiptIndex::requires_original) {
            return Ok(());
        }
        self.snapshot_receipts().and_then(|index| {
            index.require_pending_content(next)?;
            index.require_mount_namespace(next, self.config.max_entries)
        }).map_err(|source| Error::SnapshotSource(Box::new(
            SnapshotSourceError::WorktreeMounts(Box::new(source.into())),
        )))
    }
}
