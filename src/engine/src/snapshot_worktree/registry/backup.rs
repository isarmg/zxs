//! Whole Backup currently copies FILE histories, not immutable .zxs nodes.
//! This is a deny-only boundary over the existing complete receipt index. It
//! creates no directory/source/transfer authority and never scans local names.
use super::{Publication, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, SnapshotSourceError};
use crate::{Error, State};

fn registration(source: RegistrationError) -> Error {
    Error::SnapshotSource(Box::new(SnapshotSourceError::BackupRegistration(source.into())))
}

pub(crate) fn require_backup_snapshot_scope(state: &State) -> crate::Result<()> {
    let index = ReceiptIndex::capture(state).map_err(registration)?;
    // Completion IDs are reserved before their publications exist. Inspect
    // the actual validated state of each node, not `completions` membership.
    for node in index.nodes.values() {
        if matches!(node.publication, Publication::Complete(_)) {
            return Err(Error::SnapshotSource(Box::new(
                SnapshotSourceError::BackupRegisteredSnapshot(node.plan.allocated.completion),
            )));
        }
    }
    for node in index.commits.values() {
        if matches!(node.node, Publication::Complete(_)) {
            return Err(Error::SnapshotSource(Box::new(
                SnapshotSourceError::BackupRegisteredSnapshot(node.plan.node_publication),
            )));
        }
    }
    // An independently reserved but unpublished instance is not a snapshot.
    // Its unfinished ownership must nevertheless not be exported as complete.
    if let Some(operation) = index.nodes.keys().next() {
        return Err(registration(RegistrationError::Pending(*operation)));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/backup_snapshot_scope_tests.rs"]
mod tests;
