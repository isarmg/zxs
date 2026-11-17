//! Replay just the namespace of a native content publication from its original
//! sealed input. FILE payload/route and physical directory admission stay with
//! the ordinary Candidate validators and EXCL preparation.
use crate::business::namespace::{self, NamespaceDraft};
use crate::engine::FreshLayoutError as Failure;
use crate::{
    Commit, Entry, NamespaceEntryKind, NamespaceOperationKind, OperationResult, Result, State,
};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{EntryId, RepositoryId};

pub(crate) fn introduced(
    state: &State,
    result: &OperationResult,
    before: &Commit,
    after: &Commit,
    maximum: usize,
) -> Result<(RepositoryId, Vec<Entry>)> {
    let source = crate::persist::namespace_directory::workspace::decode(
        state, result, after.entries.len().min(maximum),
    )?;
    let history = state.histories.get(&after.history_tree_id).ok_or(Failure::NamespaceSource)?;
    let repository = state.repositories.get(&history.repository_id)
        .filter(|repo| repo.file_manager_root).ok_or(Failure::NamespaceSource)?;
    if result.commit_id.as_deref() != Some(after.commit_id.as_str())
        || result.branch_id.as_deref() != Some(repository.main_branch_id.as_str())
        || result.history_tree_id.as_deref() != Some(after.history_tree_id.as_str())
        || after.parent_commit_ids != [before.commit_id.clone()]
        || source.request().expected_head != before.commit_id
        || after.operation_id != result.operation_id || after.sequence != result.sequence
        || after.history_tree_id != before.history_tree_id || after.kind != "content"
        || after.namespace_deletions.is_some() || after.provenance.is_some()
        || after.author != source.request().principal || after.message != source.request().message
        || source.operations().iter().any(|op| op.kind() != NamespaceOperationKind::Mkdir)
    {
        return Err(Failure::NamespaceSource.into());
    }
    let previous_ids = before.entries.iter().map(|entry| entry.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut allocated = BTreeSet::new();
    if source.directories().iter().any(|id| {
        previous_ids.contains(id.to_string().as_str()) || !allocated.insert(*id)
    }) {
        return Err(Failure::NamespaceSource.into());
    }
    let mut ids = source.directories().iter();
    let mut next_id = |prefix: &str| -> Result<String> {
        if prefix != "entry" {
            return Err(Failure::NamespaceSource.into());
        }
        ids.next().map(ToString::to_string).ok_or_else(|| Failure::NamespaceSource.into())
    };
    let mut published_paths = BTreeMap::new();
    for entry in &after.entries {
        published_paths.entry(entry.path.key()).or_insert(entry);
    }
    let mut draft = NamespaceDraft::new(before.entries.clone());
    let mut items = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for file in source.files() {
        if !items.insert(file.item_id) || !paths.insert(file.path.key()) {
            return Err(Failure::NamespaceSource.into());
        }
        let previous = draft.get_key(file.path.key()).cloned();
        if previous.as_ref().is_some_and(|entry| entry.kind != NamespaceEntryKind::File) {
            return Err(Failure::NamespaceSource.into());
        }
        let location = previous.as_ref().map(|entry| &entry.path).unwrap_or(&file.path);
        if previous.as_ref().is_some_and(|entry| state.versions.get(&entry.version_id)
            .is_some_and(|version| version.content_hash == file.hash
                && version.logical_size == file.size))
        {
            continue;
        }
        draft.ensure_parents_with_ids(location, &mut next_id)?;
        let published = published_paths.get(location.key()).copied()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .ok_or(Failure::NamespaceSource)?;
        let version = state.versions.get(&published.version_id).ok_or(Failure::NamespaceSource)?;
        let entry_id = previous.as_ref().map(|entry| entry.entry_id.parse::<EntryId>())
            .transpose()?.unwrap_or(file.item_id);
        if published.entry_id != entry_id.to_string() || published.path != *location
            || published.size != file.size || version.logical_size != file.size
            || version.content_hash != file.hash || version.file_id != published.file_id
            || version.version_id != published.version_id
            || version.history_tree_id != after.history_tree_id
            || version.version_sequence != after.sequence
            || previous.as_ref().is_some_and(|entry| entry.file_id != published.file_id)
        {
            return Err(Failure::NamespaceSource.into());
        }
        draft.replace(published.clone());
    }
    let mut expected = draft.into_entries();
    let deleted = namespace::apply_workspace_namespace_with_ids(
        &mut expected, &before.entries, source.operations(), &mut next_id,
    )?;
    namespace::validate_entries(&mut expected, maximum)?;
    if !deleted.is_empty() || ids.next().is_some() || expected != after.entries {
        return Err(Failure::NamespaceSource.into());
    }
    let introduced = expected.into_iter().filter(|entry| {
        entry.kind == NamespaceEntryKind::Directory
            && !previous_ids.contains(entry.entry_id.as_str())
    }).collect::<Vec<_>>();
    let actual = introduced.iter().map(|entry| entry.entry_id.parse::<EntryId>())
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    if actual != allocated {
        return Err(Failure::NamespaceSource.into());
    }
    Ok((repository.repository_id.parse()?, introduced))
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/workspace_directory_source_tests.rs"]
mod tests;
