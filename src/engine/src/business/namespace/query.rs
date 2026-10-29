//! Point queries over a published, canonical paged namespace. Mutable drafts
//! do not implement this interface: appends and renames can temporarily break
//! order. Lookup descends the per-directory pages, binary searching an index
//! level before one page is read; it never folds every stored display path.
use super::*;

/// One published commit's namespace, queried through the engine's bounded
/// object cache. It is neither a second index nor an authorization grant.
pub struct NamespaceTree<'a> {
    engine: &'a Engine,
    namespace: storage_contract::CommitNamespace,
}

impl NamespaceTree<'_> {
    /// Resolve an equivalent spelling, including case-folded and NFD queries.
    /// This is lookup, not validation of a new display path.
    pub fn get(&self, path: &str) -> Result<Option<Entry>> {
        #[cfg(test)]
        tests::record_normalization();
        self.get_key(&name_policy::path_key(path))
    }

    /// Resolve an already normalized full path key. No Unicode work is needed
    /// on stored entries: pages record each component's lookup key.
    pub fn get_key(&self, key: &str) -> Result<Option<Entry>> {
        self.engine.with_namespace_source(|source| {
            crate::namespace::NamespaceView::new(&self.namespace, source).resolve_key(key)
        })
    }

    /// Materialize the complete canonical entry list. Whole-tree by
    /// definition; point queries use `get`/`get_key` instead.
    pub fn entries(&self) -> Result<Vec<Entry>> {
        self.engine.namespace_entries(&self.namespace)
    }

    /// One bounded page of a directory's direct children in component-key
    /// order, materialized with absolute paths. `after` is the last seen
    /// component lookup key; None when the key does not name a directory.
    /// Index-first: the cursor is located before any page body is read.
    pub fn directory_page(
        &self,
        directory_key: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Option<Vec<Entry>>> {
        self.engine.with_namespace_source(|source| {
            crate::namespace::NamespaceView::new(&self.namespace, source).directory_page_entries(
                directory_key,
                after,
                limit,
            )
        })
    }

    /// Recorded recursive totals `(entries, files)` of one directory (the
    /// empty key is the root). O(directory level), never a tree expansion.
    pub fn directory_totals(&self, directory_key: &str) -> Result<Option<(u64, u64)>> {
        self.engine.with_namespace_source(|source| {
            crate::namespace::NamespaceView::new(&self.namespace, source)
                .directory_totals(directory_key)
        })
    }

    /// The first entry carrying `file_id` in canonical order: an early-stop
    /// walk for namespaces without a path hint (file-branch heads are tiny).
    pub fn find_file(&self, file_id: &str) -> Result<Option<Entry>> {
        self.engine.with_namespace_source(|source| {
            crate::namespace::NamespaceView::new(&self.namespace, source).find_file(file_id)
        })
    }

    /// Stream every entry in canonical order without materializing the list;
    /// the callback returns whether to continue.
    pub fn walk(&self, mut visit: impl FnMut(&Entry) -> crate::Result<bool>) -> Result<()> {
        self.engine.with_namespace_source(|source| {
            crate::namespace::NamespaceView::new(&self.namespace, source).walk(|location, entry| {
                visit(&crate::namespace::materialized_entry(
                    location.clone(),
                    entry,
                ))
            })
        })
    }

    /// The recorded total entry count of this namespace.
    pub fn total_entries(&self) -> u64 {
        self.namespace.entries
    }
}

impl Engine {
    /// Lookup of a commit in this Engine's published State only.
    /// This does not validate branch/history scope and is not authorization.
    pub fn commit_namespace(&self, commit_id: &str) -> Option<NamespaceTree<'_>> {
        self.state
            .commits
            .get(commit_id)
            .map(|commit| NamespaceTree {
                engine: self,
                namespace: commit.namespace.clone(),
            })
    }

    /// The current published namespace with the same admission and errors as
    /// `list_tree`, but with paged normalized-key lookup.
    pub fn namespace_tree(&self, branch_id: &str) -> Result<NamespaceTree<'_>> {
        let namespace = self.tree_commit(branch_id)?.namespace.clone();
        Ok(NamespaceTree {
            engine: self,
            namespace,
        })
    }

    /// A selected published namespace. Foreign commits and migrated branches
    /// retain the `list_tree_at` errors; no per-request tree scan.
    pub fn namespace_tree_at(&self, branch_id: &str, commit_id: &str) -> Result<NamespaceTree<'_>> {
        let namespace = self.tree_commit_at(branch_id, commit_id)?.namespace.clone();
        Ok(NamespaceTree {
            engine: self,
            namespace,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/namespace/query_tests.rs"]
mod tests;
