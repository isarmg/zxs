//! The current read basis comes only from the independently completed commit.
use crate::snapshot_worktree::registry::PublishedWorktree;
use storage_contract::snapshot_worktree::SnapshotHandle;

impl PublishedWorktree<'_> {
    pub(in crate::snapshot_worktree) fn published_node(&self) -> Option<SnapshotHandle> {
        match (self.publication, self.added_publication) {
            (Some(record), None) => Some(record.plan.handle_at(record.routing.directory.branch())),
            (None, Some(record)) => Some(record.plan.handle()),
            (None, None) | (Some(_), Some(_)) => None,
        }
    }
}
