//! Closed before/after proof for displaced undo. The planner checks its
//! metadata-only draft before granting the narrow frozen-occurrence exception.
use super::*;
use crate::{Event, OperationResult};
use storage_contract::MapChanges;

pub(in crate::file_deletion) fn original(
    state: &State,
    node: &FileDeletionNode,
) -> Result<Option<DisplacedOccurrence>> {
    let receipt = state
        .operations
        .get(&node.operation_id)
        .ok_or_else(invalid)?;
    if receipt.kind == crate::business::FILE_HISTORY_MOVE_KIND {
        crate::business::admit_file_history_move_displaced(state, receipt, node)
    } else {
        Ok(None)
    }
}

fn node(result: &json_codec::Value) -> Result<FileDeletionNode> {
    Ok(json_codec::from_value(
        result
            .get("removed_deletion_node")
            .ok_or_else(invalid)?
            .clone(),
    )?)
}

fn expected_draft(
    previous: &State,
    actual: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    node: &FileDeletionNode,
    evidence: &DisplacedOccurrence,
    request: RestorePublication<'_>,
) -> Result<State> {
    let mut expected = previous.clone();
    expected.begin_tracking();
    crate::container_storage::capture_names(&mut expected);
    if actual.sequence <= previous.sequence {
        return Err(invalid());
    }
    expected.sequence = actual.sequence;
    expected.business_generation = previous
        .business_generation
        .checked_add(1)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "business generation exhausted"))?;
    restore(previous, &mut expected, current, node, evidence, request)?;
    if expected.file_deletions.remove(&node.file_id).as_ref() != Some(node) {
        return Err(invalid());
    }
    Ok(expected)
}

pub(crate) fn verify_plan(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    kind: &str,
    request: RestorePublication<'_>,
    result: &json_codec::Value,
) -> Result<bool> {
    if kind != "file.delete.undo" {
        return Ok(false);
    }
    let node = node(result)?;
    let Some(evidence) = original(previous, &node)? else {
        return Ok(false);
    };
    if *result != json!({"restored_file_id": node.file_id, "removed_deletion_node": node}) {
        return Err(invalid());
    }
    let expected = expected_draft(previous, next, current, &node, &evidence, request)?;
    // Includes every schema field, old receipt, event, frozen file, Git index
    // and unrelated namespace. This exception cannot authorize arbitrary writes.
    if expected != *next {
        return Err(invalid());
    }
    Ok(true)
}

fn new_undo<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut found = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before == after
            || !before
                .into_iter()
                .chain(after)
                .any(|op| op.kind == "file.delete.undo")
        {
            return Ok(());
        }
        let op = after.ok_or_else(invalid)?;
        if before.is_some() || op.operation_id != *id || found.replace(op).is_some() {
            return Err(invalid());
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        MapChanges::Keys(keys) => {
            for id in keys.iter() {
                inspect(id)?;
            }
        }
        MapChanges::All => {
            for id in previous
                .operations
                .keys()
                .chain(next.operations.keys())
                .collect::<std::collections::BTreeSet<_>>()
            {
                inspect(id)?;
            }
        }
    }
    Ok(found)
}

pub(crate) struct RestoredNamespace {
    pub(crate) repository_id: String,
    pub(crate) main_branch_id: String,
}

pub(crate) fn verify_publication(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
) -> Result<Option<RestoredNamespace>> {
    let Some(receipt) = new_undo(previous, next)? else {
        return Ok(None);
    };
    let node = node(&receipt.result)?;
    let Some(evidence) = original(previous, &node)? else {
        return Ok(None);
    };
    if super::super::publication::admit_receipt(next, receipt, true)? != node
        || receipt.sequence != next.sequence
    {
        return Err(invalid());
    }
    let principal = next
        .operation_principals
        .get(&receipt.operation_id)
        .ok_or_else(invalid)?;
    let mut expected = expected_draft(
        previous,
        next,
        current,
        &node,
        &evidence,
        RestorePublication {
            operation_id: receipt.operation_id,
            principal,
        },
    )?;
    crate::container_storage::capture_changed_names(previous, &mut expected);
    expected
        .operations
        .insert(receipt.operation_id, receipt.clone());
    expected
        .operation_principals
        .insert(receipt.operation_id, principal.clone());
    expected.events.push(Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: receipt.kind.clone(),
        object_id: receipt.operation_id.to_string(),
    });
    if expected != *next {
        return Err(invalid());
    }
    let repository = &previous.repositories[&node.repository_id];
    Ok(Some(RestoredNamespace {
        repository_id: repository.repository_id.clone(),
        main_branch_id: repository.main_branch_id.clone(),
    }))
}

/// After later writes/another deletion, replay only the immutable undo evidence.
/// Current selection, path and branch HEAD are deliberately not historical proof.
pub(crate) fn admit_retained_undo(
    state: &State,
    receipt: &OperationResult,
    node: &FileDeletionNode,
) -> Result<()> {
    let Some(evidence) = original(state, node)? else {
        return Ok(());
    };
    let original = original_namespace(state, node, &evidence)?;
    let id = identity(receipt.operation_id, "commit", "namespace")?;
    let restored = state.commits.get(&id).ok_or_else(invalid)?;
    if restored.parent_commit_ids.len() != 1 {
        return Err(invalid());
    }
    let previous = state
        .commits
        .get(&restored.parent_commit_ids[0])
        .ok_or_else(invalid)?;
    if previous.history_tree_id != node.history_tree_id
        || previous.sequence >= receipt.sequence
        || previous.entries.iter().any(|entry| {
            entry.lookup_key() == node.path.key()
                || entry.entry_id == evidence.displaced.entry_id
                || entry.kind == NamespaceEntryKind::File && entry.file_id == node.file_id
        })
    {
        return Err(invalid());
    }
    parent_chain(&original.entries, &previous.entries, &evidence.displaced)?;
    let principal = state
        .operation_principals
        .get(&receipt.operation_id)
        .ok_or_else(invalid)?;
    let mut entries = previous.entries.clone();
    entries.push(evidence.displaced);
    crate::business::validate_entries(&mut entries, usize::MAX)?;
    let expected = commit(
        previous,
        entries,
        receipt.sequence,
        RestorePublication {
            operation_id: receipt.operation_id,
            principal,
        },
    )?;
    if expected != *restored {
        return Err(invalid());
    }
    Ok(())
}
