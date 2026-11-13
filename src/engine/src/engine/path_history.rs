//! Typed, read-only access to permanent path facts for bounded audit consumers.
use crate::engine::history_window::HistoryCancellationCheckpoint;
use crate::{Engine, HistoryWindowCancellation, Result};
use storage_contract::file_lifecycle::PathLocation;
use storage_contract::{
    BranchId, EntryId, FileId, FilePathEventId, HistoryTreeId, OperationId, Principal,
    Sequence, Timestamp, VersionId,
};

/// One admitted local path fact, independent of the file's content graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePathChange {
    pub event_id: FilePathEventId,
    pub operation_id: OperationId,
    pub file_id: FileId,
    pub entry_id: EntryId,
    pub history_tree_id: HistoryTreeId,
    pub branch_id: BranchId,
    pub before_version_id: VersionId,
    pub after_version_id: VersionId,
    pub before: PathLocation,
    pub after: PathLocation,
    pub sequence: Sequence,
    pub occurred_at: Timestamp,
    pub author: Principal,
}

impl Engine {
    /// Scan the permanent ledger once without retaining an unbounded result.
    /// Consumers must discard their accumulated projection if admission fails;
    /// a later receipt can invalidate the whole observation. Historical copied
    /// facts are validated, but are not relabeled as new local activity.
    pub fn visit_file_path_changes(
        &self,
        cancellation: &HistoryWindowCancellation,
        mut visit: impl FnMut(FilePathChange),
    ) -> Result<()> {
        cancellation.checkpoint()?;
        crate::persist::path_changes::visit_ledger(
            &self.state,
            || cancellation.checkpoint(),
            |fact| {
                visit(FilePathChange {
                    event_id: fact.event_id,
                    operation_id: fact.operation_id,
                    file_id: fact.file_id,
                    entry_id: fact.entry_id,
                    history_tree_id: fact.history_tree_id,
                    branch_id: fact.branch_id,
                    before_version_id: fact.before_version_id,
                    after_version_id: fact.after_version_id,
                    before: fact.before,
                    after: fact.after,
                    sequence: fact.sequence,
                    occurred_at: fact.occurred_at,
                    author: fact.author,
                });
                Ok(())
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/path_history_tests.rs"]
mod tests;
