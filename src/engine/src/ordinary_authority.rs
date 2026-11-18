//! Negative namespace-domain boundary shared by the actual native writers and
//! readers. Passing it is not authority to serve or repair: the original
//! Candidate, exact root registration and FILE-specific verifier must still pass.
use crate::{Error, ErrorCode, OperationResult, Result, State};
use json_codec::Value;

fn unsupported() -> Error {
    Error::new(ErrorCode::StateInvalid, "metadata is not ordinary FILE namespace authority")
}

pub(crate) fn kind(kind: &str) -> Result<()> {
    // Stopping an ordinary filesystem import does not create a directory
    // repository. Grouped FILE lifecycle has its own native file.delete.batch
    // decision; it is not the initialized directory Git deletion marker.
    if kind.starts_with("snapshot.")
        || (kind.starts_with("directory.")
            && kind != "directory.import.stop")
        || matches!(kind, "branch.create" | "history.split" | "history.historical_save" | "history.restore")
    {
        return Err(unsupported());
    }
    Ok(())
}

pub(crate) fn plan(kind: &str) -> Result<()> {
    // This is the ordinary local maintenance plan, not membership/recovery
    // plans: those have their own types and do not pass through this reader.
    if kind == "purge" { Ok(()) } else { Err(unsupported()) }
}

// This is the complete executable local plan shape, not an authorization to
// purge. The producer, retained cancellations, and cold plan files use it.
pub(crate) fn plan_input(kind_name: &str, input: &Value) -> Result<()> {
    plan(kind_name)?;
    let fields = input.as_object().filter(|fields| fields.len() == 2).ok_or_else(unsupported)?;
    let history = fields.get("history_tree_id").and_then(Value::as_str).ok_or_else(unsupported)?;
    storage_contract::validate_id(history, "history").map_err(|_| unsupported())?;
    let versions = fields.get("version_ids").and_then(Value::as_array)
        .filter(|versions| !versions.is_empty()).ok_or_else(unsupported)?;
    let mut last = None;
    for value in versions {
        let version = value.as_str().ok_or_else(unsupported)?;
        if last.is_some_and(|previous| previous >= version) {
            return Err(unsupported());
        }
        storage_contract::validate_id(version, "ver").map_err(|_| unsupported())?;
        last = Some(version);
    }
    Ok(())
}

fn original_fields(kind_name: &str, result: &Value) -> Result<()> {
    kind(kind_name)?;
    // These original envelopes have untyped JSON payloads. A changed kind,
    // even on a fully checksummed record or degraded outcome, must not hide
    // their independent directory authority. Inspect protocol fields only,
    // never user metadata, display names, arbitrary nested objects or paths.
    if result.get("restored_path_audit").is_some()
        || result.get("snapshot_worktree").is_some()
        || result.get("directory_branch_ids").is_some()
        || matches!(result.pointer("/directory/format").and_then(Value::as_str),
            Some("directory_draft_decision" | "directory_draft_unchanged"))
    {
        return Err(unsupported());
    }
    let has_origin = result.get("file_manager_root_origin").is_some_and(Value::is_object);
    if kind_name == "repository.create" {
        if !has_origin
            || result.pointer("/created_repository/file_manager_root").and_then(Value::as_bool)
                != Some(true)
        {
            return Err(unsupported());
        }
    } else if result.get("file_manager_root_origin").is_some() {
        return Err(unsupported());
    }
    Ok(())
}

pub(crate) fn operation(receipt: &OperationResult) -> Result<()> {
    original_fields(&receipt.kind, &receipt.result)
}

// Native radix encoding calls this only on the actual changed/new row; cold
// admission independently checks all decoded rows even on an archive or an
// entirely foreign object graph.
pub(crate) fn encoded_row(field: &str, value: &Value) -> Result<()> {
    match field {
        "repositories" if value.get("file_manager_root").and_then(Value::as_bool) != Some(true) => {
            Err(unsupported())
        }
        "operations" => original_fields(
            value.get("kind").and_then(Value::as_str).ok_or_else(unsupported)?,
            value.get("result").ok_or_else(unsupported)?,
        ),
        "events" => kind(value.get("kind").and_then(Value::as_str).ok_or_else(unsupported)?),
        "plans" => plan_input(
            value.get("kind").and_then(Value::as_str).ok_or_else(unsupported)?,
            value.get("input").ok_or_else(unsupported)?,
        ),
        _ => Ok(()),
    }
}

/// Constant time, including before incremental encoding or scoped projection.
/// In-memory callers must never succeed by dropping a table they populated.
pub(crate) fn tables(state: &State) -> Result<()> {
    if !state.file_write_origins.is_empty()
        || !state.directory_branches.is_empty() || !state.directory_git_repositories.is_empty()
        || !state.directory_trees.is_empty() || !state.directory_tree_selections.is_empty()
        || !state.directory_selections.is_empty() || !state.directory_selection_generations.is_empty()
    {
        return Err(unsupported());
    }
    Ok(())
}

pub(crate) fn decoded_records(state: &State) -> Result<()> {
    tables(state)?;
    for receipt in state.operations.values() { operation(receipt)?; }
    for event in state.events.iter() { kind(&event.kind)?; }
    for pending in state.plans.values() { plan_input(&pending.kind, &pending.input)?; }
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/engine/unit/ordinary_authority_tests.rs"]
mod tests;
