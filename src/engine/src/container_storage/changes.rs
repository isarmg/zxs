//! Container projection candidates from exact transaction-local map writes.
use crate::State;
use std::collections::BTreeSet;
use storage_contract::MapChanges;

fn visit_changes<'a>(
    changes: MapChanges,
    all: impl Iterator<Item = &'a String>,
    inspect: impl FnMut(&String),
) {
    match changes {
        MapChanges::Keys(keys) => keys.iter().for_each(inspect),
        MapChanges::All => all.for_each(inspect),
    }
}

pub(super) fn changed_files(previous: &State, next: &State) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    // Keys are conservative writes, not semantic changes. Keep each projection's
    // original predicate; lost lineage still reads both authoritative maps.
    macro_rules! inspect {
        ($field:ident, |$key:ident| $body:block) => {
            visit_changes(
                next.$field.changes_since(&previous.$field),
                previous.$field.keys().chain(next.$field.keys()),
                |$key| $body,
            )
        };
    }
    macro_rules! inserted_or_removed {
        ($field:ident) => {
            inspect!($field, |id| {
                if (!previous.$field.contains_key(id) || !next.$field.contains_key(id))
                    && let Some(node) = next.$field.get(id).or_else(|| previous.$field.get(id))
                {
                    files.insert(node.file_id.clone());
                }
            });
        };
    }
    inserted_or_removed!(file_metadata_nodes);
    inserted_or_removed!(file_branch_merges);
    inserted_or_removed!(file_path_nodes);
    macro_rules! file_values {
        ($($field:ident),* $(,)?) => {$(
            inspect!($field, |file| {
                if previous.$field.get(file) != next.$field.get(file) {
                    files.insert(file.clone());
                }
            });
        )*};
    }
    file_values!(
        file_backup_paths,
        file_backup_policies,
        file_locations,
        file_names,
        file_notes,
        file_selections,
    );
    inspect!(file_path_notes, |id| {
        if previous.file_path_notes.get(id) != next.file_path_notes.get(id) {
            if let Some(node) = next
                .file_path_nodes
                .get(id)
                .or_else(|| previous.file_path_nodes.get(id))
            {
                files.insert(node.file_id.clone());
            } else if let Some(node) = next
                .file_metadata_nodes
                .get(id)
                .or_else(|| previous.file_metadata_nodes.get(id))
            {
                files.insert(node.file_id.clone());
            }
        }
    });
    inspect!(file_node_parents, |id| {
        if previous.file_node_parents.get(id) != next.file_node_parents.get(id)
            && let Some(version) = next.versions.get(id).or_else(|| previous.versions.get(id))
        {
            files.insert(version.file_id.clone());
        }
    });
    inspect!(file_contexts, |key| {
        if previous.file_contexts.get(key) != next.file_contexts.get(key) {
            for (state, context) in previous
                .file_contexts
                .get(key)
                .map(|context| (previous, context))
                .into_iter()
                .chain(next.file_contexts.get(key).map(|context| (next, context)))
            {
                if let Some(file) = state
                    .branches
                    .get(&context.branch_id)
                    .and_then(|branch| branch.file_id.as_ref())
                {
                    files.insert(file.clone());
                }
            }
        }
    });
    inspect!(version_notes, |id| {
        if previous.version_notes.get(id) != next.version_notes.get(id)
            && let Some(version) = next.versions.get(id).or_else(|| previous.versions.get(id))
        {
            files.insert(version.file_id.clone());
        }
    });
    changed_versions(previous, next, &mut files);
    // Pending references are projected even before a branch owns a FileVersion.
    inspect!(branches, |id| {
        let before = previous.branches.get(id);
        let after = next.branches.get(id);
        let unchanged = match (before, after) {
            (Some(before), Some(after)) => {
                before.pending_file_versions == after.pending_file_versions
                    && before.file_node_id == after.file_node_id
                    && before.name == after.name
                    && before.state == after.state
                    && before.history_tree_id == after.history_tree_id
            }
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        };
        if !unchanged {
            for branch in before.into_iter().chain(after) {
                files.extend(branch.pending_file_versions.keys().cloned());
            }
        }
    });
    files
}

fn changed_versions(previous: &State, next: &State, files: &mut BTreeSet<String>) {
    let changes = [
        next.versions.changes_since(&previous.versions),
        next.placements.changes_since(&previous.placements),
        next.content_states.changes_since(&previous.content_states),
    ];
    let mut inspect = |id: &String| {
        let changed_placement = match (previous.placements.get(id), next.placements.get(id)) {
            (Some(before), Some(after)) => {
                before.route_slot_ids != after.route_slot_ids
                    || before.binding_generations != after.binding_generations
                    || before.primary_slot_id != after.primary_slot_id
                    || before.target_copies != after.target_copies
            }
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
        };
        if !previous.versions.contains_key(id)
            || !next.versions.contains_key(id)
            || changed_placement
            || previous.content_states.get(id) != next.content_states.get(id)
        {
            // The original scan visits both records when an existing identity
            // changes owner. Preserve that behavior, including cold inputs.
            for version in previous
                .versions
                .get(id)
                .into_iter()
                .chain(next.versions.get(id))
            {
                files.insert(version.file_id.clone());
            }
        }
    };
    let mut keys = BTreeSet::new();
    for change in &changes {
        match change {
            MapChanges::Keys(changed) => keys.extend(changed.iter()),
            MapChanges::All => {
                previous
                    .versions
                    .keys()
                    .chain(next.versions.keys())
                    .for_each(inspect);
                return;
            }
        }
    }
    keys.into_iter().for_each(&mut inspect);
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/changes_tests.rs"]
mod tests;
