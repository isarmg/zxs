//! Additional row dependencies of the file-local database, beyond the existing
//! JSON/name projection. Exact map keys remain incremental; cold All scans are
//! intentionally conservative and derived from both admitted States.
use crate::State;
use std::collections::BTreeSet;
use storage_contract::MapChanges;

pub(crate) fn affected(previous: &State, next: &State) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    macro_rules! inspect {
        ($field:ident, |$id:ident| $body:block) => {
            let mut inspect = |$id: &String| $body;
            match next.$field.changes_since(&previous.$field) {
                MapChanges::Keys(keys) => keys.iter().for_each(&mut inspect),
                MapChanges::All => previous
                    .$field
                    .keys()
                    .chain(next.$field.keys())
                    .for_each(&mut inspect),
            }
        };
    }
    macro_rules! file_values {
        ($($field:ident),* $(,)?) => {$(
            inspect!($field, |id| {
                if previous.$field.get(id) != next.$field.get(id) { files.insert(id.clone()); }
            });
        )*};
    }
    file_values!(file_deletions);
    inspect!(branches, |id| {
        if previous.branches.get(id) != next.branches.get(id) {
            for branch in previous
                .branches
                .get(id)
                .into_iter()
                .chain(next.branches.get(id))
            {
                files.extend(branch.file_id.iter().cloned());
            }
        }
    });
    inspect!(file_branch_aliases, |id| {
        if previous.file_branch_aliases.get(id) != next.file_branch_aliases.get(id) {
            for state in [previous, next] {
                files.extend(
                    state
                        .branches
                        .get(id)
                        .and_then(|branch| branch.file_id.clone()),
                );
            }
        }
    });
    inspect!(merge_links, |id| {
        if previous.merge_links.get(id) != next.merge_links.get(id) {
            for state in [previous, next] {
                if let Some(link) = state.merge_links.get(id) {
                    for branch in [&link.source_branch_id, &link.target_branch_id] {
                        files.extend(
                            state
                                .branches
                                .get(branch)
                                .and_then(|branch| branch.file_id.clone()),
                        );
                    }
                }
            }
        }
    });
    let mut operations = BTreeSet::new();
    for change in [
        next.operations.changes_since(&previous.operations),
        next.operation_principals
            .changes_since(&previous.operation_principals),
    ] {
        match change {
            MapChanges::Keys(keys) => operations.extend(keys.iter().copied()),
            MapChanges::All => operations.extend(
                previous
                    .operations
                    .keys()
                    .chain(next.operations.keys())
                    .copied(),
            ),
        }
    }
    for id in operations {
        for receipt in previous
            .operations
            .get(&id)
            .into_iter()
            .chain(next.operations.get(&id))
        {
            if let crate::state_projection::lifecycles::DeletionOwner::File(file) =
                crate::state_projection::lifecycles::owner(receipt)
            {
                files.insert(file.to_owned());
            }
            if let crate::state_projection::lifecycles::DeletionOwner::File(file) =
                crate::state_projection::lifecycles::history_copy_owner(receipt)
            {
                files.insert(file.to_owned());
            }
        }
    }
    // Retain whole immutable node rows. Cold All inputs cannot assume that an
    // old identity still denotes the same exact metadata.
    macro_rules! versions {
        ($field:ident) => {
            inspect!($field, |id| {
                if previous.$field.get(id) != next.$field.get(id) {
                    for state in [previous, next] {
                        files.extend(
                            state
                                .versions
                                .get(id)
                                .map(|version| version.file_id.clone()),
                        );
                    }
                }
            });
        };
    }
    versions!(versions);
    macro_rules! nodes {
        ($field:ident) => {
            inspect!($field, |id| {
                if previous.$field.get(id) != next.$field.get(id) {
                    for node in previous
                        .$field
                        .get(id)
                        .into_iter()
                        .chain(next.$field.get(id))
                    {
                        files.insert(node.file_id.clone());
                    }
                }
            });
        };
    }
    nodes!(file_path_nodes);
    nodes!(file_metadata_nodes);
    nodes!(file_branch_merges);
    files
}
