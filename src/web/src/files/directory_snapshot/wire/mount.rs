//! Mount outcomes carry the entire original. A later name, revision or status
//! cannot settle an earlier operation, even when it addresses the same handle.
use super::{LookupResult, Publication, decode, invalid, object_field};
use crate::api;
use json_codec::Value;
use storage_contract::{
    RepositoryId,
    snapshot_worktree::mount_change::{MountAction, MountChange, MountChangeReceipt, MountOutcome, RestoreNaming},
};

pub(super) fn request_shape(value: &Value) -> Result<(), api::Error> {
    object_field(value, "target")?;
    object_field(value, "expected")?;
    let action = object_field(value, "action")?;
    match action.get("kind").and_then(Value::as_str) {
        Some("hide") => { object_field(action, "committed")?; }
        Some("restore") => { object_field(action, "naming")?; }
        _ => return Err(invalid("unknown original mount action")),
    }
    Ok(())
}

pub(in crate::files::directory_snapshot) fn receipt(value: Value) -> Result<MountChangeReceipt, api::Error> {
    request_shape(object_field(&value, "request")?)?;
    object_field(&value, "outcome")?;
    let receipt: MountChangeReceipt = decode(value)?;
    let valid = match (&receipt.request.action, &receipt.outcome) {
        (_, MountOutcome::Pending {}) | (MountAction::Hide { .. }, MountOutcome::Hidden {}) => true,
        (MountAction::Restore { naming: RestoreNaming::Explicit { name: asked }, .. }, MountOutcome::Restored { name }) => asked == name,
        (MountAction::Restore { naming: RestoreNaming::OriginalOrAutomatic {}, .. }, MountOutcome::Restored { .. }) => true,
        _ => false,
    };
    if !valid {
        return Err(invalid("mount result differs from the exact original action or explicit name"));
    }
    Ok(receipt)
}

pub(in crate::files::directory_snapshot) fn settled(
    repository: RepositoryId,
    original: &MountChange,
    response: Result<Value, api::Error>,
) -> Result<Option<Publication>, api::Error> {
    let receipt = receipt(response?)?;
    if &receipt.request != original {
        return Err(invalid("mount receipt differs from the entire original request"));
    }
    match receipt.outcome {
        MountOutcome::Pending {} => Ok(None),
        _ => Ok(Some(Publication::MountChanged { repository, receipt })),
    }
}

pub(super) fn lookup(repository: RepositoryId, original: &MountChange, value: Value) -> Result<LookupResult, api::Error> {
    Ok(match settled(repository, original, Ok(value))? {
        Some(done) => LookupResult::Complete(done),
        None => LookupResult::Pending,
    })
}
