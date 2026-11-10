//! Borrowed current visibility. Retained namespaces and directory layouts keep
//! the raw rows; only current reads hide an admitted active container marker.
use super::DirectoryGitRepository;
use crate::{Entry, NamespaceEntryKind, TrackedMap};

/// Visibility filtering only. Callers must separately admit the retained
/// namespace, its repository and their current authorization.
pub fn current_entry_visible(
    repositories: &TrackedMap<DirectoryGitRepository>,
    repository: &str,
    entries: &[Entry],
    entry: &Entry,
) -> bool {
    if repositories.is_empty() {
        return true;
    }
    let mut directories = Vec::new();
    let mut key = entry.lookup_key();
    if entry.kind == NamespaceEntryKind::Directory {
        directories.push(entry.entry_id.as_str());
        if let Some(git) = repositories.get(&entry.entry_id) {
            return directories
                .iter()
                .all(|id| !git.deletions.contains_key(*id));
        }
    }
    while let Some((parent, _)) = key.rsplit_once('/') {
        let index = entries
            .binary_search_by(|entry| entry.lookup_key().cmp(parent))
            .expect("invariant: an admitted namespace has all directory parents");
        let directory = &entries[index];
        directories.push(directory.entry_id.as_str());
        if let Some(git) = repositories.get(&directory.entry_id) {
            // Creation refuses initialized children, and the freeze guard bars
            // initializing one inside a deleted ordinary subtree. The nearest
            // initialized owner therefore owns every possible marker here.
            return directories
                .iter()
                .all(|id| !git.deletions.contains_key(*id));
        }
        key = parent;
    }
    repositories.get(repository).is_none_or(|git| {
        directories
            .iter()
            .all(|id| !git.deletions.contains_key(*id))
    })
}
