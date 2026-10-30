//! Bounded selected facts from one admitted publication, outside the writer lock.
use super::*;
use crate::{
    DirectoryBranchChoice, DirectorySelectionBranch, DirectorySelectionSnapshot,
    ReadDirectorySelection, SelectedEntryFact, backend_directory_selection::SelectedDirectoryEntry,
};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry as MapEntry};

pub(super) fn read(
    view: storage_engine::PublishedReadView,
    selection: &ReadDirectorySelection,
    control: &crate::HistoryReadControl,
) -> Result<DirectorySelectionSnapshot, ApiError> {
    file_manager::activity::checkpoint(control)?;
    let state = view.state();
    let repo = state
        .repositories
        .get(selection.repository_id())
        .expect("invariant: admitted repository belongs to the captured publication");
    let key = selection.scope().map_or("", name_policy::LookupPath::key);
    let observe = || control.checkpoint().map_err(storage_engine::Error::from);
    // Resolve before inspecting any source, including an empty selection, so
    // namespace failures keep their precedence over source/head conflicts.
    let namespace = (!key.is_empty()).then(|| {
        let branch = state
            .branches
            .get(&repo.main_branch_id)
            .expect("invariant: admitted repository references its main branch");
        view.commit_namespace(&branch.head_commit_id, &observe)
            .expect("invariant: admitted branch references its head commit")
    });
    let scope = super::directory_scopes::ScopeMatch::resolve(repo, Some(key), namespace)?;
    let mut branches = Vec::with_capacity(selection.sources().len());
    for source in selection.sources() {
        file_manager::activity::checkpoint(control)?;
        let branch = state
            .directory_branches
            .get(&source.directory_branch_id)
            .filter(|branch| {
                scope.matches(branch) && branch.head_tree_id == source.expected_tree_id
            })
            .ok_or_else(|| {
                ApiError::conflict(
                    storage_contract::ErrorCode::ListingChanged,
                    "source scope changed; refresh before preparing a batch",
                )
            })?;
        let current = state.directory_selections.get(&branch.directory_id)
            == Some(&branch.directory_branch_id);
        let (file_count, entries) =
            selected_entry_facts(&branch.entries, selection.entries(), control)?;
        branches.push(DirectorySelectionBranch {
            summary: DirectoryBranchChoice::capture_counted(branch, current, file_count),
            entries,
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
    control: &crate::HistoryReadControl,
) -> Result<(usize, Vec<SelectedEntryFact>), ApiError> {
    file_manager::activity::checkpoint(control)?;
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
    let mut file_count = 0;
    for entry in entries {
        file_manager::activity::checkpoint(control)?;
        if entry.kind == storage_contract::NamespaceEntryKind::File {
            file_count += 1;
        }
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
    let facts = selections
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
        .collect();
    file_manager::activity::checkpoint(control)?;
    Ok((file_count, facts))
}
#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/file_manager/scope_query_tests.rs"]
mod tests;
