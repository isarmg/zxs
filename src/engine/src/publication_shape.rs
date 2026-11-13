//! Shared complete-State comparisons for operation replay and publication shapes.
use crate::{Error, OperationId, OperationResult, Result, State};
use storage_contract::{MapChanges, SequenceChanges, TrackedOperations};

#[derive(Clone, Copy)]
pub(crate) enum ChangedTable {
    ContentStates,
    FileDeletions,
    FilePathNotes,
}

// Compare the union of two independently tracked write sets. A missing lineage
// uses the complete table, including rows absent from either proposed outcome.
macro_rules! same_maps {
    ($previous:ident, $expected:ident, $actual:ident; $($field:ident),* $(,)?) => {
        $(
            let same = match (
                $expected.$field.changes_since(&$previous.$field),
                $actual.$field.changes_since(&$previous.$field),
            ) {
                (MapChanges::Keys(left), MapChanges::Keys(right)) => left
                    .iter()
                    .chain(right.iter())
                    .all(|key| $expected.$field.get(key) == $actual.$field.get(key)),
                _ => $expected.$field == $actual.$field,
            };
            if !same {
                return false;
            }
        )*
    };
}

// The field declaration covers the complete State. Each operation names only
// its mutable tables; provenance decides whether comparison visits writes or rows.
macro_rules! state_tables {
    ($($field:ident => $variant:ident,)*) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(crate) enum Table { $($variant,)* }

        pub(crate) fn unchanged_except(
            previous: &State,
            next: &State,
            mutable: &[Table],
        ) -> bool {
            let State {
                sequence: _, business_generation: _, topology_generation: _,
                control_auth: _, operations: _, operation_principals: _, events: _,
                $($field: _,)*
            } = next;
            $(
                if !mutable.contains(&Table::$variant) {
                    let same = match next.$field.changes_since(&previous.$field) {
                        MapChanges::Keys(keys) => keys.iter().all(|key| {
                            next.$field.get(key) == previous.$field.get(key)
                        }),
                        MapChanges::All => next.$field == previous.$field,
                    };
                    if !same {
                        return false;
                    }
                }
            )*
            true
        }

        // Compare the complete schema, using exact tracked keys on the live path.
        // Cold deserialization has no tracking authority and compares full maps.
        pub(crate) fn same_transition(previous: &State, expected: &State, actual: &State) -> bool {
            same_maps!(previous, expected, actual; $($field,)* operations, operation_principals);
            let events = match (
                expected.events.changes_since(&previous.events),
                actual.events.changes_since(&previous.events),
            ) {
                (SequenceChanges::Append { start: left }, SequenceChanges::Append { start: right })
                    if left == previous.events.len() && right == previous.events.len() => {
                    expected.events[left..] == actual.events[right..]
                }
                _ => expected.events == actual.events,
            };
            events
                && expected.sequence == actual.sequence
                && expected.business_generation == actual.business_generation
                && expected.topology_generation == actual.topology_generation
                && expected.control_auth == actual.control_auth
        }
    };
}
state_tables! {
    container_edits => ContainerEdits,
    container_tags => ContainerTags,
    repositories => Repositories,
    histories => Histories,
    branches => Branches,
    file_selections => FileSelections,
    directory_branches => DirectoryBranches,
    directory_git_repositories => DirectoryGitRepositories,
    directory_trees => DirectoryTrees,
    file_write_origins => FileWriteOrigins,
    file_contexts => FileContexts,
    file_path_nodes => FilePathNodes,
    file_metadata_nodes => FileMetadataNodes,
    file_branch_merges => FileBranchMerges,
    file_branch_aliases => FileBranchAliases,
    file_node_parents => FileNodeParents,
    directory_tree_selections => DirectoryTreeSelections,
    file_locations => FileLocations,
    file_backup_paths => FileBackupPaths,
    file_backup_policies => FileBackupPolicies,
    directory_selections => DirectorySelections,
    directory_selection_generations => DirectorySelectionGenerations,
    file_selection_generations => FileSelectionGenerations,
    file_notes => FileNotes,
    version_notes => VersionNotes,
    file_names => FileNames,
    commits => Commits,
    versions => Versions,
    placements => Placements,
    merge_links => MergeLinks,
    history_boundaries => HistoryBoundaries,
    plans => Plans,
    placement_rules => PlacementRules,
    protection_policies => ProtectionPolicies,
    content_states => ContentStates,
    file_deletions => FileDeletions,
    file_path_notes => FilePathNotes,
}

pub(crate) fn verify(
    previous: &State,
    next: &State,
    receipt: &OperationResult,
    changed: ChangedTable,
    invalid: fn() -> Error,
) -> Result<()> {
    let table = match changed {
        ChangedTable::ContentStates => Table::ContentStates,
        ChangedTable::FileDeletions => Table::FileDeletions,
        ChangedTable::FilePathNotes => Table::FilePathNotes,
    };
    if !unchanged_except(previous, next, &[table]) {
        return Err(invalid());
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
