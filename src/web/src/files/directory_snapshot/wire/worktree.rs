//! Original requests, not names or a later HEAD, identify worktree outcomes.
use super::{Command, LookupResult, Publication, Record, decode, invalid, object_field};
use crate::api;
use json_codec::Value;
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::outcome::{
    WorktreeCommitOutcome, WorktreeCommitReceipt, WorktreeCreationOutcome, WorktreeCreationReceipt,
};
use storage_contract::snapshot_worktree::{SnapshotHandle, WorktreeHandle};

pub(super) fn created(repository: RepositoryId, value: Value) -> Result<Publication, api::Error> {
    let target = decode(value)?;
    Ok(Publication::WorktreeCreated { repository, target })
}

pub(super) fn committed(
    repository: RepositoryId,
    source: SnapshotHandle,
    target: WorktreeHandle,
    value: Value,
) -> Result<Publication, api::Error> {
    let snapshot: SnapshotHandle = decode(value)?;
    // This production slice continues the source branch. The node completion
    // is independent of the reserve operation; never compare those two IDs.
    if snapshot.repository_id != source.repository_id
        || snapshot.branch_id != source.branch_id
        || snapshot.node_id == source.node_id
    {
        return Err(invalid(
            "the worktree result differs from its exact source branch",
        ));
    }
    Ok(Publication::WorktreeCommitted {
        repository,
        target,
        snapshot,
    })
}

pub(super) fn lookup(record: &Record, value: Value) -> Result<LookupResult, api::Error> {
    match &record.command {
        Command::CreateWorktree {
            repository,
            request,
        } => {
            object_field(object_field(&value, "request")?, "source")?;
            let result = object_field(&value, "result")?;
            if result.get("state").and_then(Value::as_str) == Some("complete") {
                object_field(result, "target")?;
            }
            let receipt: WorktreeCreationReceipt = decode(value)?;
            if receipt.repository_id != *repository || receipt.request != *request {
                return Err(invalid(
                    "the worktree creation receipt differs from the original",
                ));
            }
            match receipt.result {
                WorktreeCreationOutcome::Pending {} => Ok(LookupResult::Pending),
                WorktreeCreationOutcome::Complete { target } => {
                    Ok(LookupResult::Complete(Publication::WorktreeCreated {
                        repository: *repository,
                        target,
                    }))
                }
            }
        }
        Command::CommitWorktree {
            repository,
            source,
            request,
        } => {
            let original = object_field(&value, "request")?;
            object_field(original, "target")?;
            object_field(original, "expected")?;
            let result = object_field(&value, "result")?;
            if result.get("state").and_then(Value::as_str) == Some("complete") {
                object_field(result, "snapshot")?;
            }
            let receipt: WorktreeCommitReceipt = decode(value)?;
            if receipt.repository_id != *repository || receipt.request != *request {
                return Err(invalid(
                    "the worktree commit receipt differs from the original",
                ));
            }
            match receipt.result {
                WorktreeCommitOutcome::Pending {} => Ok(LookupResult::Pending),
                WorktreeCommitOutcome::Complete { snapshot } => {
                    if snapshot.repository_id != source.repository_id
                        || snapshot.branch_id != source.branch_id
                        || snapshot.node_id == source.node_id
                    {
                        return Err(invalid(
                            "the committed node differs from the original branch",
                        ));
                    }
                    Ok(LookupResult::Complete(Publication::WorktreeCommitted {
                        repository: *repository,
                        target: request.target,
                        snapshot,
                    }))
                }
            }
        }
        Command::Commit { .. } => record.result(value).map(LookupResult::Complete),
        Command::RegisterRoot { .. } | Command::Initialize { .. } => {
            Err(invalid("this original request has no outcome lookup"))
        }
    }
}
