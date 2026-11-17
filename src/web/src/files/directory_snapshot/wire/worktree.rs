//! Original requests, not names or a later HEAD, identify worktree outcomes.
use super::{Command, LookupResult, Publication, Record, decode, invalid, object_field};
use crate::api;
use json_codec::Value;
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::creation::WorktreeCreationRequest;
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
    // A completed commit may continue or fork the exact held source. The core
    // owns that decision; a later HEAD is not evidence of this request's result.
    // Node completion is independent of the original operation ID.
    if snapshot.repository_id != source.repository_id || snapshot.node_id == source.node_id {
        return Err(invalid(
            "the worktree result must be a new node in its exact source repository",
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
        } => creation_lookup(
            *repository,
            WorktreeCreationRequest::Named(request.clone()),
            value,
        ),
        Command::AllocateWorktree {
            repository,
            request,
        } => creation_lookup(
            *repository,
            WorktreeCreationRequest::Automatic(request.clone()),
            value,
        ),
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
                        || snapshot.node_id == source.node_id
                    {
                        return Err(invalid(
                            "the committed node differs from the original repository",
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

fn creation_lookup(
    repository: RepositoryId,
    request: WorktreeCreationRequest,
    value: Value,
) -> Result<LookupResult, api::Error> {
    object_field(object_field(&value, "request")?, "source")?;
    let result = object_field(&value, "result")?;
    if result.get("state").and_then(Value::as_str) == Some("complete") {
        object_field(result, "target")?;
    }
    let receipt: WorktreeCreationReceipt = decode(value)?;
    if receipt.repository_id != repository || receipt.request != request {
        return Err(invalid(
            "the worktree creation receipt differs from the original",
        ));
    }
    match receipt.result {
        WorktreeCreationOutcome::Pending {} => Ok(LookupResult::Pending),
        WorktreeCreationOutcome::Complete { target } => {
            Ok(LookupResult::Complete(Publication::WorktreeCreated {
                repository,
                target,
            }))
        }
    }
}
