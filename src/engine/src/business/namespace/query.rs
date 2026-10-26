//! Borrowed queries over a published, canonical namespace. Mutable builders do
//! not implement this interface: appends and renames can temporarily break order.
use super::*;

/// The immutable, lookup-key-sorted entries of one published commit.
///
/// Obtain this view from [`Engine::namespace_tree`] or
/// [`Engine::namespace_tree_at`]. It borrows the engine; it is neither a second
/// index nor an authorization grant. Lookup folds only the query, never every
/// stored display path. Display spellings, stable identities and aliases remain
/// unchanged.
#[derive(Clone, Copy)]
pub struct NamespaceTree<'a> {
    entries: &'a [Entry],
}

impl<'a> NamespaceTree<'a> {
    // Only this module constructs views. Engine publication validates canonical
    // form through HistoryManifest::from_state; admitted history uses the same
    // validator. No arbitrary Commit, mutable State or entry slice is accepted
    // by the public or crate-wide API, and queries never repeat that validation.
    fn from_published_entries(entries: &'a [Entry]) -> Self {
        Self { entries }
    }

    /// Borrow the canonical order, without allocating or copying entries.
    pub fn entries(self) -> &'a [Entry] {
        self.entries
    }

    /// Resolve an equivalent spelling, including case-folded and NFD queries.
    /// This is lookup, not validation of a new display path.
    pub fn get(self, path: &str) -> Option<&'a Entry> {
        #[cfg(test)]
        tests::record_normalization();
        self.get_key(&name_policy::path_key(path))
    }

    /// Resolve an already normalized full path key. No Unicode work is needed.
    pub fn get_key(self, key: &str) -> Option<&'a Entry> {
        self.entries
            .binary_search_by(|entry| {
                #[cfg(test)]
                tests::record_comparison();
                entry.lookup_key().cmp(key)
            })
            .ok()
            .map(|index| &self.entries[index])
    }
}

impl Engine {
    /// Internal lookup of a commit in this Engine's published State only.
    /// This does not validate branch/history scope and is not authorization.
    pub(crate) fn commit_namespace(&self, commit_id: &str) -> Option<NamespaceTree<'_>> {
        self.state
            .commits
            .get(commit_id)
            .map(|commit| NamespaceTree::from_published_entries(&commit.entries))
    }

    /// Borrow the current published namespace with the same admission and
    /// errors as `list_tree_ref`, but with logarithmic normalized-key lookup.
    pub fn namespace_tree(&self, branch_id: &str) -> Result<NamespaceTree<'_>> {
        self.list_tree_ref(branch_id)
            .map(NamespaceTree::from_published_entries)
    }

    /// Borrow a selected published namespace. Foreign commits and migrated
    /// branches retain the `list_tree_at_ref` errors; no per-request tree scan.
    pub fn namespace_tree_at(&self, branch_id: &str, commit_id: &str) -> Result<NamespaceTree<'_>> {
        self.list_tree_at_ref(branch_id, commit_id)
            .map(NamespaceTree::from_published_entries)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/namespace/query_tests.rs"]
mod tests;
