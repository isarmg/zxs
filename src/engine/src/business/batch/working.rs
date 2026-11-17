//! New copy requests consume committed entries, never a shared working body.
//! The caller has already admitted every original scope and exact source row.
use super::BatchScope;
use crate::business::{file_draft, namespace};
use crate::{Entry, NamespaceEntryKind, Result, State};
use std::collections::BTreeSet;
use storage_contract::file_container::ContainerParent;
use storage_contract::{EntryId, RepositoryId};

pub(super) fn require_committed_source(
    state: &State,
    scope: &BatchScope,
    source: &Entry,
    current: &[Entry],
) -> Result<()> {
    let members = || scope.branch().entries.iter().filter(move |entry| {
        entry.lookup_key() == source.lookup_key()
            || (source.kind == NamespaceEntryKind::Directory
                && namespace::is_descendant(entry.lookup_key(), source.lookup_key()))
    });
    // This is the actual selected committed subtree, including secondary
    // sources for Separate. An unrelated current alias is not a source.
    for entry in members().filter(|entry| entry.kind == NamespaceEntryKind::File) {
        file_draft::require_committed_transfer(state, &entry.file_id)?;
    }
    if source.kind != NamespaceEntryKind::Directory || !scope.is_active(state) {
        return Ok(());
    }
    let repository = scope.branch().repository_id.parse::<RepositoryId>()?;
    if !state.file_containers.values().any(|record| {
        record.namespace.repository == repository && record.working.unborn().is_some()
            && !record.namespace.parent.is_worktree()
    }) {
        return Ok(());
    }
    // Unborn files have no committed Entry and would otherwise disappear from
    // a current directory copy. Only current directory identities in the exact
    // selected subtree can own them. A historical same-name directory, a
    // worktree mount, and a directory outside this selection do not qualify.
    let current_directories = current.iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
        .map(|entry| (entry.entry_id.as_str(), entry.lookup_key()))
        .collect::<BTreeSet<_>>();
    if !current_directories.contains(&(source.entry_id.as_str(), source.lookup_key())) {
        return Ok(());
    }
    let selected_directories = members()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory
            && current_directories.contains(&(entry.entry_id.as_str(), entry.lookup_key())))
        .map(|entry| entry.entry_id.parse::<EntryId>())
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    for (file, record) in state.file_containers.iter() {
        if record.namespace.repository == repository && record.working.unborn().is_some()
            && let ContainerParent::Directory(parent) = &record.namespace.parent
            && selected_directories.contains(parent)
        {
            file_draft::require_committed_transfer(state, &file.to_string())?;
        }
    }
    Ok(())
}
