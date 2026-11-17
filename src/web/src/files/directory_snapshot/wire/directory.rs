//! Public directory read projection. It is not proof that the caller's full
//! retained write description matches the permanent command.
use super::{decode, invalid, object_field};
use crate::api;
use json_codec::Value;
use storage_contract::CommitState;
use storage_contract::snapshot_worktree::{
    SnapshotHandle,
    draft_commit::{CommitDirectoryDrafts, DirectoryDraftOutcome, DirectoryDraftReceipt},
};

/// The scoped GET proves the account, original directory handle, operation
/// and actual node. It deliberately does *not* echo the ordered selected rows
/// or note; a positive GET cannot authenticate a recovered browser record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::files::directory_snapshot) struct DirectoryObservation {
    pub(in crate::files::directory_snapshot) result: DirectoryDraftOutcome,
    pub(in crate::files::directory_snapshot) degraded: bool,
}
impl DirectoryObservation {
    pub(in crate::files::directory_snapshot) fn snapshot(&self) -> SnapshotHandle {
        match self.result {
            DirectoryDraftOutcome::Changed { snapshot }
            | DirectoryDraftOutcome::Unchanged { snapshot } => snapshot,
        }
    }
}
pub(super) fn observe(
    request: &CommitDirectoryDrafts,
    value: Value,
) -> Result<DirectoryObservation, api::Error> {
    object_field(object_field(&value, "result")?, "snapshot")?;
    let receipt: DirectoryDraftReceipt = decode(value)?;
    if receipt.operation_id != request.operation_id {
        return Err(invalid("the receipt belongs to another directory commit"));
    }
    match receipt.result {
        DirectoryDraftOutcome::Changed { snapshot }
            if snapshot.publication == request.operation_id
                && snapshot.repository_id == request.source.repository_id
                && snapshot.branch_id == request.source.branch_id
                && snapshot.node_id != request.source.node_id => {}
        DirectoryDraftOutcome::Unchanged { snapshot } if snapshot == request.source => {}
        _ => {
            return Err(invalid(
                "the directory receipt differs from the original branch, node or change",
            ));
        }
    }
    let degraded = match receipt.commit_state {
        CommitState::Published | CommitState::Acked => false,
        CommitState::PublishedDegraded => true,
        _ => {
            return Err(invalid(
                "publication is not confirmed; retain the original request",
            ));
        }
    };
    Ok(DirectoryObservation {
        result: receipt.result,
        degraded,
    })
}
