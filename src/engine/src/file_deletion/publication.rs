//! Closed deletion write sets are verified before publication and on registry replay.
use super::*;
use crate::{CommitState, FileDeletionNode, OperationId, OperationResult, ProtectionAtPublish};
use json_codec::json;
use storage_contract::{MapChanges, SequenceChanges, TrackedOperations};

mod cold;
pub(crate) use cold::admit_current;

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "file deletion publication differs from its exact lifecycle request",
    )
}

pub(crate) fn verify(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
) -> Result<()> {
    if crate::business::object_move::verify_publication(previous, next, current)? {
        return Ok(());
    }
    if super::directory::verify_publication(previous, next, current)? {
        return Ok(());
    }
    if super::occurrence::verify_publication(previous, next, current)?.is_some() {
        return Ok(());
    }
    let mut changed = std::collections::BTreeSet::new();
    let inspect = |key: &str, changed: &mut std::collections::BTreeSet<String>| {
        if previous.file_deletions.get(key) != next.file_deletions.get(key) {
            changed.insert(key.into());
        }
    };
    match next.file_deletions.changes_since(&previous.file_deletions) {
        MapChanges::Keys(keys) => {
            for key in keys.iter() {
                inspect(key, &mut changed);
            }
        }
        MapChanges::All => {
            for key in previous
                .file_deletions
                .keys()
                .chain(next.file_deletions.keys())
                .collect::<std::collections::BTreeSet<_>>()
            {
                inspect(key, &mut changed);
            }
        }
    }
    let mut lifecycle = None;
    let mut inspect_receipt = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before == after
            || !before.into_iter().chain(after).any(|receipt| {
                matches!(receipt.kind.as_str(), "file.delete" | "file.delete.undo")
                    || receipt.kind == super::batch::KIND
            })
        {
            return Ok(());
        }
        let receipt = after.ok_or_else(invalid)?;
        if receipt.operation_id != *id || before.is_some() || lifecycle.replace(receipt).is_some() {
            return Err(invalid());
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        MapChanges::Keys(keys) => {
            for id in keys.iter() {
                inspect_receipt(id)?;
            }
        }
        MapChanges::All => {
            for id in previous
                .operations
                .keys()
                .chain(next.operations.keys())
                .collect::<std::collections::BTreeSet<_>>()
            {
                inspect_receipt(id)?;
            }
        }
    }
    if changed.is_empty() {
        return if lifecycle.is_none() {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let receipt = lifecycle.ok_or_else(invalid)?;
    verify_shape(previous, next, receipt)?;
    if receipt.kind == super::batch::KIND {
        return super::batch::verify(previous, next, current, receipt, &changed);
    }
    if changed.len() != 1 {
        return Err(invalid());
    }
    let file = changed.first().expect("invariant: one changed deletion");
    let (node, undo) = match (
        previous.file_deletions.get(file),
        next.file_deletions.get(file),
    ) {
        (None, Some(node)) => (node, false),
        (Some(node), None) => (node, true),
        _ => return Err(invalid()),
    };
    if receipt.kind
        != if undo {
            "file.delete.undo"
        } else {
            "file.delete"
        }
    {
        return Err(invalid());
    }
    if undo {
        super::directory::require_individual_undo(previous, &node.file_id)?;
        preflight::require_undo_scope(previous, current, node)?;
    }
    verify_receipt(previous, next, node, receipt, undo)?;
    if !undo {
        verify_frontier(previous, current, node)?;
    }
    Ok(())
}

pub(in crate::file_deletion) fn verify_frontier(
    previous: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    node: &FileDeletionNode,
) -> Result<()> {
    let repository = previous
        .repositories
        .get(&node.repository_id)
        .ok_or_else(invalid)?;
    let branch = previous
        .branches
        .get(&repository.main_branch_id)
        .ok_or_else(invalid)?;
    let commit = previous
        .commits
        .get(&branch.head_commit_id)
        .ok_or_else(invalid)?;
    let entries = match current {
        Some(view) => {
            view.require_branch_write(previous, &branch.branch_id)?;
            view.branch_entries(&branch.branch_id, &commit.commit_id)?
        }
        None => &commit.entries,
    };
    let repository_id = HistoryRepositoryId::parse(&node.repository_id)?;
    let file_id = HistoryFileId::parse(&node.file_id)?;
    let path = name_policy::LookupPath::parse(node.path.as_str())?;
    preflight::require_scope_writable(
        previous,
        current,
        repository,
        entries,
        &node.file_id,
        &HistoryWindowCancellation::default(),
    )?;
    let capture = preflight::capture(
        previous,
        repository,
        entries,
        FileDeletionTarget {
            repository_id: &repository_id,
            file_id: &file_id,
            path: &path,
        },
        usize::MAX,
        &HistoryWindowCancellation::default(),
    )?;
    let heads = capture
        .branches()
        .iter()
        .map(|head| (head.branch().branch_id.clone(), head.node_id().to_owned()))
        .collect();
    if node.branch_heads != heads
        || node.parent_node_ids != capture.parent_node_ids()
        || node.history_tree_id != repository.history_tree_id
    {
        return Err(invalid());
    }
    Ok(())
}

fn verify_receipt(
    previous: &State,
    next: &State,
    node: &FileDeletionNode,
    receipt: &OperationResult,
    undo: bool,
) -> Result<()> {
    if admit_receipt(next, receipt, undo)? != *node
        || receipt.sequence != next.sequence
        || previous.operations.contains_key(&receipt.operation_id)
    {
        return Err(invalid());
    }
    Ok(())
}

/// One envelope rule serves hot publication and cold lifecycle replay. A past
/// node's frontier may legitimately differ from current heads after its undo.
pub(in crate::file_deletion) fn admit_receipt(
    state: &State,
    receipt: &OperationResult,
    undo: bool,
) -> Result<FileDeletionNode> {
    let node: FileDeletionNode = json_codec::from_value(
        receipt
            .result
            .get(if undo {
                "removed_deletion_node"
            } else {
                "deletion_node"
            })
            .ok_or_else(invalid)?
            .clone(),
    )?;
    admit_node_shape(&node)?;
    let input = if undo {
        json!({"file_id": node.file_id, "deletion_node_id": node.node_id})
    } else {
        json!({"repository_id": node.repository_id, "file_id": node.file_id, "path": node.path.key()})
    };
    let principal = admit_envelope(state, receipt, &input, if undo { 5 } else { 4 })?;
    if receipt.kind
        != if undo {
            "file.delete.undo"
        } else {
            "file.delete"
        }
        || node.sequence == 0
        || node.sequence > receipt.sequence
        || (!undo
            && (node.operation_id != receipt.operation_id
                || node.sequence != receipt.sequence
                || node.author != principal))
        || (undo
            && (node.sequence == receipt.sequence
                || receipt.result["restored_file_id"].as_str() != Some(node.file_id.as_str())))
    {
        return Err(invalid());
    }
    if undo {
        super::occurrence::admit_retained_undo(state, receipt, &node)?;
    }
    Ok(node)
}

pub(in crate::file_deletion) fn admit_envelope<'a>(
    state: &'a State,
    receipt: &OperationResult,
    input: &json_codec::Value,
    fields: usize,
) -> Result<&'a str> {
    let principal = state
        .operation_principals
        .get(&receipt.operation_id)
        .ok_or_else(invalid)?;
    Engine::require_actor(principal)?;
    if receipt.sequence == 0
        || receipt.sequence > state.sequence
        || receipt.request_digest != Engine::native_request_digest(&receipt.kind, principal, input)?
        || receipt.commit_id.is_some()
        || receipt.history_tree_id.is_some()
        || receipt.branch_id.is_some()
        || !matches!(
            receipt.commit_state,
            CommitState::Published | CommitState::PublishedDegraded
        )
        || receipt.protection_at_publish != ProtectionAtPublish::NotApplicable
        || receipt.result["version_protection"] != json!([])
        || !receipt.result["timestamp"]
            .as_str()
            .is_some_and(|value| value.parse::<u64>().is_ok())
        || !receipt.result["membership_epoch"]
            .as_str()
            .is_some_and(|value| value.parse::<u64>().is_ok())
        || receipt
            .result
            .as_object()
            .is_none_or(|result| result.len() != fields)
    {
        return Err(invalid());
    }
    Ok(principal)
}

/// Shared wire-shape rule; current-frontier and publication authority are separate.
pub(crate) fn admit_node_shape(node: &FileDeletionNode) -> Result<()> {
    storage_contract::validate_id(&node.node_id, "fdel")?;
    HistoryFileId::parse(&node.file_id)?;
    HistoryRepositoryId::parse(&node.repository_id)?;
    storage_contract::history_window::HistoryTreeId::parse(&node.history_tree_id)?;
    for (branch, head) in &node.branch_heads {
        storage_contract::history_window::HistoryBranchId::parse(branch)?;
        storage_contract::history_window::HistoryNodeId::parse(head)?;
    }
    let parents = node
        .branch_heads
        .values()
        .collect::<std::collections::BTreeSet<_>>();
    if node.branch_heads.is_empty() || parents.into_iter().ne(node.parent_node_ids.iter()) {
        return Err(invalid());
    }
    Ok(())
}

fn appended<T: PartialEq>(
    previous: &TrackedOperations<T>,
    next: &TrackedOperations<T>,
    operation: OperationId,
) -> bool {
    if previous.contains_key(&operation)
        || !next.contains_key(&operation)
        || next.len() != previous.len() + 1
    {
        return false;
    }
    let unchanged = |key: &OperationId| *key == operation || previous.get(key) == next.get(key);
    match next.changes_since(previous) {
        MapChanges::Keys(keys) => keys.iter().all(unchanged),
        MapChanges::All => previous.keys().chain(next.keys()).all(unchanged),
    }
}

fn verify_shape(previous: &State, next: &State, receipt: &OperationResult) -> Result<()> {
    macro_rules! unchanged {
        ($($field:ident),* $(,)?) => {{
            let State { sequence: _, business_generation: _, topology_generation: _, control_auth: _,
                file_deletions: _, operations: _, operation_principals: _, events: _, $($field: _,)* } = next;
            $(let same = match next.$field.changes_since(&previous.$field) {
                MapChanges::Keys(keys) => keys.iter().all(|key| next.$field.get(key) == previous.$field.get(key)),
                MapChanges::All => next.$field == previous.$field,
            }; if !same { return Err(invalid()); })*
        }};
    }
    unchanged!(
        repositories,
        histories,
        branches,
        file_selections,
        directory_branches,
        directory_git_repositories,
        directory_trees,
        file_write_origins,
        file_contexts,
        file_path_nodes,
        file_metadata_nodes,
        file_branch_merges,
        file_branch_aliases,
        file_node_parents,
        file_path_notes,
        directory_tree_selections,
        file_locations,
        file_backup_paths,
        file_backup_policies,
        directory_selections,
        directory_selection_generations,
        file_selection_generations,
        file_notes,
        version_notes,
        file_names,
        commits,
        versions,
        placements,
        merge_links,
        history_boundaries,
        content_states,
        plans,
        placement_rules,
        protection_policies
    );
    if next.sequence <= previous.sequence
        || previous.business_generation.checked_add(1) != Some(next.business_generation)
        || next.topology_generation != previous.topology_generation
        || next.control_auth != previous.control_auth
        || !appended(&previous.operations, &next.operations, receipt.operation_id)
        || !appended(
            &previous.operation_principals,
            &next.operation_principals,
            receipt.operation_id,
        )
        || next.events.len() != previous.events.len() + 1
    {
        return Err(invalid());
    }
    match next.events.changes_since(&previous.events) {
        SequenceChanges::Append { start } if start == previous.events.len() => (),
        SequenceChanges::All if next.events[..previous.events.len()] == previous.events[..] => (),
        _ => return Err(invalid()),
    }
    let event = next.events.last().ok_or_else(invalid)?;
    if event.operation_id != receipt.operation_id
        || event.sequence != next.sequence
        || event.kind != receipt.kind
        || event.object_id != receipt.operation_id.to_string()
    {
        return Err(invalid());
    }
    Ok(())
}
