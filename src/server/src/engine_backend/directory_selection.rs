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
    let scopes = super::directory_scopes::matching_unordered(
        engine,
        repo,
        Some(selection.scope().map_or("", name_policy::LookupPath::key)),
    )?
    .map(|branch| (branch.directory_branch_id.as_str(), branch))
    .collect::<BTreeMap<_, _>>();
    let mut branches = Vec::with_capacity(selection.sources().len());
    let mut selected = None;
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
            entries: selected
                .get_or_insert_with(|| SelectedEntries::new(selection.entries()))
                .facts(&branch.entries),
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
struct SelectedEntries<'a> {
    selections: &'a [SelectedDirectoryEntry],
    ids: BTreeSet<&'a str>,
    keys: BTreeSet<(&'a str, &'a str)>,
}
impl<'a> SelectedEntries<'a> {
    fn new(selections: &'a [SelectedDirectoryEntry]) -> Self {
        Self {
            selections,
            ids: selections
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect(),
            keys: selections
                .iter()
                .map(|entry| (entry.entry_id.as_str(), entry.path.key()))
                .collect(),
        }
    }
    fn facts(&self, entries: &[storage_contract::Entry]) -> Vec<SelectedEntryFact> {
        let mut matches = BTreeMap::new();
        let mut exact = BTreeMap::new();
        for entry in entries {
            if self.ids.contains(entry.entry_id.as_str()) {
                match matches.entry(entry.entry_id.as_str()) {
                    MapEntry::Vacant(slot) => {
                        slot.insert(IdentityMatch::Unique(entry));
                    }
                    MapEntry::Occupied(mut slot) => {
                        slot.insert(IdentityMatch::Ambiguous);
                    }
                }
                let key = (entry.entry_id.as_str(), entry.lookup_key());
                if self.keys.contains(&key) {
                    exact.insert(key, entry);
                }
            }
        }
        self.selections
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
}
#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/file_manager/scope_query_tests.rs"]
mod tests;
