//! Exact single-operation publication shape for table-local metadata changes.
use crate::{Error, OperationId, OperationResult, Result, State};
use storage_contract::{MapChanges, SequenceChanges, TrackedOperations};

#[derive(Clone, Copy)]
pub(crate) enum ChangedTable {
    ContentStates,
    FileDeletions,
    FilePathNotes,
}

pub(crate) fn verify(
    previous: &State,
    next: &State,
    receipt: &OperationResult,
    changed: ChangedTable,
    invalid: fn() -> Error,
) -> Result<()> {
    macro_rules! unchanged {
        ($($field:ident),* $(,)?) => {{
            $(
                let same = match next.$field.changes_since(&previous.$field) {
                    MapChanges::Keys(keys) => keys
                        .iter()
                        .all(|key| next.$field.get(key) == previous.$field.get(key)),
                    MapChanges::All => next.$field == previous.$field,
                };
                if !same {
                    return Err(invalid());
                }
            )*
        }};
    }
    // A single schema covers every State field. Adding a field must explicitly
    // decide whether a historical annotation is allowed to mutate it.
    macro_rules! fixed_maps {
        ($($field:ident),* $(,)?) => {{
            let State {
                sequence: _,
                business_generation: _,
                topology_generation: _,
                control_auth: _,
                content_states: _,
                file_deletions: _,
                file_path_notes: _,
                operations: _,
                operation_principals: _,
                events: _,
                $($field: _,)*
            } = next;
            unchanged!($($field),*);
        }};
    }
    fixed_maps!(
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
        plans,
        placement_rules,
        protection_policies,
    );
    match changed {
        ChangedTable::ContentStates => unchanged!(file_deletions, file_path_notes),
        ChangedTable::FileDeletions => unchanged!(content_states, file_path_notes),
        ChangedTable::FilePathNotes => unchanged!(content_states, file_deletions),
    }
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

#[cfg(test)]
#[path = "../../../test/engine/unit/publication_shape_tests.rs"]
mod tests;
