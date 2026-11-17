//! Whole Backup currently copies FILE histories, not immutable .zxs nodes.
//! This is a deny-only boundary over the existing complete receipt index. It
//! creates no directory/source/transfer authority and never scans local names.
use super::{Publication, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, SnapshotSourceError};
use crate::{Error, State};

fn registration(source: RegistrationError) -> Error {
    Error::SnapshotSource(Box::new(SnapshotSourceError::BackupRegistration(source.into())))
}

/// A standalone admitted snapshot has no observed original/outcome cursor.
/// Absence of snapshot duties is usable; indexed records require their actual
/// publication source, never a successful decode of visible receipt words.
pub(crate) fn require_backup_snapshot_scope(state: &State) -> crate::Result<()> {
    match state.operations.values().find(|receipt| ReceiptIndex::requires_original(receipt)) {
        Some(receipt) => Err(registration(
            RegistrationError::OriginalReceiptUnavailable(receipt.operation_id),
        )),
        None => Ok(()),
    }
}

impl crate::Engine {
    pub(crate) fn require_backup_snapshot_scope(&self) -> crate::Result<()> {
        let context = self.planning_context();
        match context.original_receipts {
            Some(_) => {
                let index = context.snapshot_receipts().map_err(registration)?;
                require_index(&index)
            }
            None => require_backup_snapshot_scope(context.state),
        }
    }
}

fn require_index(index: &ReceiptIndex<'_>) -> crate::Result<()> {
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
