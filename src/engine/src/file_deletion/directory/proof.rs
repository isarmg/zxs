//! The same native group evidence drives publication, cold replay and FILE audit.
//! It never fabricates a per-file receipt for a directory operation.
use super::*;
use crate::Event;
use storage_contract::MapChanges;

pub(crate) fn shape(node: &DirectoryDeletionNode) -> Result<()> {
    storage_contract::validate_id(&node.node_id, "ddel")?;
    HistoryRepositoryId::parse(&node.repository_id)?;
    storage_contract::history_window::HistoryTreeId::parse(&node.history_tree_id)?;
    storage_contract::validate_id(&node.directory_id, "entry")?;
    if node.owner_directory_id != node.repository_id {
        storage_contract::validate_id(&node.owner_directory_id, "entry")?;
    }
    storage_contract::validate_id(&node.namespace_head, "commit")?;
    storage_contract::validate_id(&node.directory_branch_id, "dbr")?;
    storage_contract::validate_id(&node.source_tree_id, "dtree")?;
    if node.node_id != identity(node.operation_id, "ddel", &node.directory_id)?
        || node.sequence == 0
        || node.author.is_empty()
        || node.files.len() > super::super::batch::MAX_FILES
    {
        return Err(invalid());
    }
    for (file, member) in &node.files {
        HistoryFileId::parse(file)?;
        storage_contract::validate_id(&member.deletion_node_id, "fdel")?;
    }
    Ok(())
}

fn immutable_frontier(state: &State, node: &FileDeletionNode) -> Result<()> {
    super::super::publication::admit_node_shape(node)?;
    for head in node.branch_heads.values() {
        let owner = state
            .versions
            .get(head)
            .map(|row| (&row.file_id, &row.history_tree_id))
            .or_else(|| {
                state
                    .file_path_nodes
                    .get(head)
                    .map(|row| (&row.file_id, &row.history_tree_id))
            })
            .or_else(|| {
                state
                    .file_metadata_nodes
                    .get(head)
                    .map(|row| (&row.file_id, &row.history_tree_id))
            });
        if owner != Some((&node.file_id, &node.history_tree_id)) {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Resolve the original creator of this exact marker. An aggregate movement
/// may contain several imported markers, so it never impersonates a one-marker
/// directory.delete receipt.
pub(crate) fn admit_original(
    state: &State,
    receipt: &OperationResult,
    node: &DirectoryDeletionNode,
) -> Result<Admitted> {
    if receipt.kind != storage_contract::file_relocation::OBJECT_MOVE_KIND {
        return admit(state, receipt);
    }
    let result = crate::business::object_move::admit_result(state, receipt)?;
    if !result
        .imported_directory_deletions
        .iter()
        .any(|original| original == node)
    {
        return Err(invalid());
    }
    shape(node)?;
    source::retained(state, node)?;
    let mut files = Vec::new();
    for (file, member) in &node.files {
        let imported = result
            .imported_deletions
            .iter()
            .find(|row| &row.file_id == file && row.node_id == member.deletion_node_id)
            .ok_or_else(invalid)?;
        immutable_frontier(state, imported)?;
        if imported.repository_id != node.repository_id
            || imported.history_tree_id != node.history_tree_id
        {
            return Err(invalid());
        }
        if member.created_by_directory {
            files.push(imported.clone());
        }
    }
    Ok(Admitted {
        node: node.clone(),
        files,
        undo: false,
    })
}

pub(crate) fn admit(state: &State, receipt: &OperationResult) -> Result<Admitted> {
    let undo = receipt.kind == UNDO;
    if !is_kind(&receipt.kind) {
        return Err(invalid());
    }
    let node: DirectoryDeletionNode = json_codec::from_value(
        receipt
            .result
            .get(if undo {
                "removed_directory_deletion"
            } else {
                "directory_deletion"
            })
            .ok_or_else(invalid)?
            .clone(),
    )?;
    let files: Vec<FileDeletionNode> = json_codec::from_value(
        receipt
            .result
            .get(if undo {
                "removed_deletion_nodes"
            } else {
                "deletion_nodes"
            })
            .ok_or_else(invalid)?
            .clone(),
    )?;
    shape(&node)?;
    receipt_budget(&receipt.result, super::super::batch::MAX_RESULT_BYTES)?;
    source::retained(state, &node)?;
    if undo {
        let input = UndoInput {
            repository_id: node.repository_id.clone(),
            owner_directory_id: node.owner_directory_id.clone(),
            directory_id: node.directory_id.clone(),
            deletion_node_id: node.node_id.clone(),
        };
        super::super::publication::admit_envelope(state, receipt, &json!(input), 6)?;
        let original = state
            .operations
            .get(&node.operation_id)
            .filter(|original| {
                matches!(
                    original.kind.as_str(),
                    DELETE | storage_contract::file_relocation::OBJECT_MOVE_KIND
                )
            })
            .ok_or_else(invalid)?;
        let original = admit_original(state, original, &node)?;
        if original.node != node
            || original.files != files
            || receipt.sequence <= node.sequence
            || receipt.result["restored_directory_id"].as_str() != Some(node.directory_id.as_str())
        {
            return Err(invalid());
        }
    } else {
        let input: DeleteInput =
            json_codec::from_value(receipt.result.get("input").ok_or_else(invalid)?.clone())?;
        let principal =
            super::super::publication::admit_envelope(state, receipt, &json!(&input), 6)?;
        if input.repository_id != node.repository_id
            || input.request.kind != DirectoryBatchKind::Delete
            || input.request.items.len() != 1
            || input.request.items[0].target.is_some()
            || node.operation_id != receipt.operation_id
            || node.sequence != receipt.sequence
            || node.author != principal
        {
            return Err(invalid());
        }
        let requested = &input.request.items[0].source;
        let expected_tree = state
            .directory_trees
            .get(&requested.expected_tree_id)
            .ok_or_else(invalid)?;
        let path = name_policy::PortablePath::parse(&requested.path)?;
        if requested.entry_id != node.directory_id
            || requested.directory_branch_id != node.directory_branch_id
            || path.key() != node.path.key()
            || !source::inherits(
                state,
                &node.directory_branch_id,
                &expected_tree.directory_branch_id,
            )
            || expected_tree.sequence >= node.sequence
        {
            return Err(invalid());
        }
        let originals = &state.commits[&node.namespace_head].entries;
        let owned = node
            .files
            .iter()
            .filter(|(_, member)| member.created_by_directory)
            .map(|(file, _)| file);
        if owned.ne(files.iter().map(|node| &node.file_id)) {
            return Err(invalid());
        }
        let mut frontier = (0, 0);
        for file in &files {
            immutable_frontier(state, file)?;
            let occurrence = source::subtree(originals, node.path.key())
                .into_iter()
                .find(|entry| {
                    entry.kind == NamespaceEntryKind::File && entry.file_id == file.file_id
                })
                .ok_or_else(invalid)?;
            if file.node_id != identity(node.operation_id, "fdel", &file.file_id)?
                || node.files[&file.file_id].deletion_node_id != file.node_id
                || file.repository_id != node.repository_id
                || file.history_tree_id != node.history_tree_id
                || file.path != occurrence.path
                || file.operation_id != node.operation_id
                || file.author != node.author
                || file.sequence != node.sequence
                || file.created_at != node.created_at
            {
                return Err(invalid());
            }
            frontier.0 += file.branch_heads.len();
            frontier.1 += file.parent_node_ids.len();
            super::super::batch::require_frontier(frontier)?;
        }
    }
    Ok(Admitted { node, files, undo })
}

pub(crate) fn node_for(
    state: &State,
    receipt: &OperationResult,
    file: &str,
) -> Result<FileDeletionNode> {
    admit(state, receipt)?
        .files
        .into_iter()
        .find(|node| node.file_id == file)
        .ok_or_else(invalid)
}

fn changed_keys<T>(
    before: &storage_contract::TrackedMap<T>,
    after: &storage_contract::TrackedMap<T>,
) -> Vec<String> {
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys.iter().cloned().collect(),
        MapChanges::All => before
            .keys()
            .chain(after.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn markers_changed(previous: &State, next: &State) -> bool {
    changed_keys(
        &previous.directory_git_repositories,
        &next.directory_git_repositories,
    )
    .iter()
    .any(|id| {
        let before = previous
            .directory_git_repositories
            .get(id)
            .map(|git| &git.deletions);
        let after = next
            .directory_git_repositories
            .get(id)
            .map(|git| &git.deletions);
        before
            .into_iter()
            .chain(after)
            .any(|nodes| !nodes.is_empty())
            && before != after
    })
}

fn new_receipt<'a>(previous: &State, next: &'a State) -> Result<Option<&'a OperationResult>> {
    let mut found = None;
    let mut inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before == after
            || !before
                .into_iter()
                .chain(after)
                .any(|receipt| is_kind(&receipt.kind))
        {
            return Ok(());
        }
        let receipt = after.ok_or_else(invalid)?;
        if before.is_some() || receipt.operation_id != *id || found.replace(receipt).is_some() {
            return Err(invalid());
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        MapChanges::Keys(keys) => keys.iter().try_for_each(&mut inspect)?,
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
    let unchanged = |id: &OperationId| {
        previous
            .operations
            .get(id)
            .is_none_or(|receipt| !is_kind(&receipt.kind))
            || previous.operation_principals.get(id) == next.operation_principals.get(id)
    };
    let valid = match next
        .operation_principals
        .changes_since(&previous.operation_principals)
    {
        MapChanges::Keys(keys) => keys.iter().all(unchanged),
        MapChanges::All => previous
            .operation_principals
            .keys()
            .chain(next.operation_principals.keys())
            .all(unchanged),
    };
    if !valid {
        return Err(invalid());
    }
    Ok(found)
}

pub(crate) fn verify_publication(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
) -> Result<bool> {
    let Some(receipt) = new_receipt(previous, next)? else {
        return if markers_changed(previous, next) {
            Err(invalid())
        } else {
            Ok(false)
        };
    };
    let admitted = admit(next, receipt)?;
    if receipt.sequence != next.sequence || next.sequence <= previous.sequence {
        return Err(invalid());
    }
    let principal = next
        .operation_principals
        .get(&receipt.operation_id)
        .ok_or_else(invalid)?;
    let mut expected = previous.clone();
    expected.begin_tracking();
    crate::container_storage::capture_names(&mut expected);
    expected.sequence = next.sequence;
    expected.business_generation = previous
        .business_generation
        .checked_add(1)
        .ok_or_else(invalid)?;
    let mut result = if admitted.undo {
        apply_undo(
            previous,
            &mut expected,
            current,
            &UndoInput {
                repository_id: admitted.node.repository_id.clone(),
                owner_directory_id: admitted.node.owner_directory_id.clone(),
                directory_id: admitted.node.directory_id.clone(),
                deletion_node_id: admitted.node.node_id.clone(),
            },
        )?
    } else {
        let input = json_codec::from_value(receipt.result["input"].clone())?;
        apply_create(
            previous,
            &mut expected,
            current,
            &input,
            Creation {
                principal,
                operation: receipt.operation_id,
                timestamp: admitted.node.created_at,
                maximum: usize::MAX,
            },
        )?
    };
    for field in ["timestamp", "membership_epoch", "version_protection"] {
        result[field] = receipt.result[field].clone();
    }
    if result != receipt.result {
        return Err(invalid());
    }
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
    Ok(true)
}
