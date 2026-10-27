//! Current immutable-tree baselines and selected facts, under one read admission.
use super::*;
use crate::{
    DirectoryBranchChoice, DirectorySelectionBranch, DirectorySelectionSnapshot,
    ReadDirectorySelection, SelectedEntryFact, backend_directory_selection::SelectedDirectoryEntry,
};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry as MapEntry};

pub(super) fn read(
    engine: &Engine,
    selection: &ReadDirectorySelection,
) -> Result<DirectorySelectionSnapshot, ApiError> {
    let repo = repository(engine, selection.repository_id())?;
    let state = engine.state();
    let scopes = super::directory_scopes::matching(
        state,
        repo,
        Some(selection.scope().map_or("", name_policy::LookupPath::key)),
    )
    .into_iter()
    .map(|branch| (branch.directory_branch_id.as_str(), branch))
    .collect::<BTreeMap<_, _>>();
    let mut branches = Vec::with_capacity(selection.sources().len());
    for source in selection.sources() {
        let branch = scopes
            .get(source.directory_branch_id.as_str())
            .filter(|branch| branch.head_tree_id == source.expected_tree_id)
            .ok_or_else(|| {
                ApiError::conflict(
                    storage_contract::ErrorCode::ListingChanged,
                    "source scope changed; refresh before preparing a batch",
                )
            })?;
        let current = state.directory_selections.get(&branch.directory_id)
            == Some(&branch.directory_branch_id);
        branches.push(DirectorySelectionBranch {
            summary: DirectoryBranchChoice::capture(branch, current),
            entries: selected_entry_facts(&branch.entries, selection.entries()),
        });
    }
    Ok(DirectorySelectionSnapshot {
        repository_id: repo.repository_id.clone(),
        branches,
    })
}
enum IdentityMatch<'a> {
    Unique(&'a storage_contract::Entry),
    Ambiguous,
}
fn selected_entry_facts(
    entries: &[storage_contract::Entry],
    selections: &[SelectedDirectoryEntry],
) -> Vec<SelectedEntryFact> {
    let ids = selections
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    let selected_keys = selections
        .iter()
        .map(|entry| (entry.entry_id.as_str(), entry.path.key()))
        .collect::<BTreeSet<_>>();
    let mut matches = BTreeMap::new();
    let mut exact = BTreeMap::new();
    for entry in entries {
        if ids.contains(entry.entry_id.as_str()) {
            match matches.entry(entry.entry_id.as_str()) {
                MapEntry::Vacant(slot) => {
                    slot.insert(IdentityMatch::Unique(entry));
                }
                MapEntry::Occupied(mut slot) => {
                    slot.insert(IdentityMatch::Ambiguous);
                }
            }
            let key = (entry.entry_id.as_str(), entry.lookup_key());
            if selected_keys.contains(&key) {
                exact.insert(key, entry);
            }
        }
    }
    selections
        .iter()
        .filter_map(|selected| {
            let chosen = match matches.get(selected.entry_id.as_str()) {
                Some(IdentityMatch::Unique(entry)) => Some(*entry),
                Some(IdentityMatch::Ambiguous) | None => exact
                    .get(&(selected.entry_id.as_str(), selected.path.key()))
                    .copied(),
            };
            chosen.map(|entry| SelectedEntryFact {
                entry_id: entry.entry_id.clone(),
                path: entry.path.as_str().into(),
                selected_path: selected.path.as_str().into(),
            })
        })
        .collect()
}
#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/file_manager/scope_query_tests.rs"]
mod tests;
