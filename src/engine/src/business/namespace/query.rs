//! Point queries over a published, canonical paged namespace. Mutable drafts
//! do not implement this interface: appends and renames can temporarily break
//! order. Lookup descends the per-directory pages, binary searching an index
//! level before one page is read; it never folds every stored display path.
use super::*;
use std::borrow::Cow;

/// One published commit's namespace, queried through the engine's bounded
/// object cache. It is neither a second index nor an authorization grant.
pub struct NamespaceTree<'a> {
    resources: &'a crate::disk::DiskResources,
    caches: &'a crate::segment_store::SegmentCaches,
    namespace: storage_contract::CommitNamespace,
    members: Cow<'a, [storage_contract::DiskConfig]>,
    observe: Option<&'a dyn Fn() -> Result<()>>,
}

impl crate::namespace::NamespaceSource for NamespaceTree<'_> {
    fn node(
        &self,
        reference: &storage_contract::NamespaceReference,
    ) -> Result<crate::namespace::NamespaceNode> {
        crate::segment_store::SegmentReader {
            resources: self.resources,
            caches: self.caches,
            members: &self.members,
            observe: self.observe,
        }
        .namespace_node(reference)
    }
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
        crate::namespace::NamespaceView::new(&self.namespace, self).resolve_key(key)
    }

    /// Test a file identity at a normalized key without materializing an owned
    /// entry. Complete stored-path validation and lookup errors are unchanged.
    pub(crate) fn contains_file_key(&self, key: &str, file_id: &str) -> Result<bool> {
        Ok(crate::namespace::NamespaceView::new(&self.namespace, self)
            .inspect_key(key, |entry| {
                matches!(entry, storage_contract::NamespacePageEntry::File {
                    file_id: actual, ..
                } if actual == file_id)
            })?
            .unwrap_or(false))
    }

    /// Test a directory at a normalized key, optionally requiring its exact
    /// identity. Missing entries and files return false; read errors propagate.
    pub fn contains_directory_key(&self, key: &str, entry_id: Option<&str>) -> Result<bool> {
        Ok(crate::namespace::NamespaceView::new(&self.namespace, self)
            .inspect_key(key, |entry| {
                matches!(entry, storage_contract::NamespacePageEntry::Directory {
                    entry_id: actual, ..
                } if entry_id.is_none_or(|expected| actual == expected))
            })?
            .unwrap_or(false))
    }

    /// Materialize the complete canonical entry list. Whole-tree by
    /// definition; point queries use `get`/`get_key` instead.
    pub fn entries(&self) -> Result<Vec<Entry>> {
        crate::namespace::NamespaceView::new(&self.namespace, self).collect_entries()
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
        crate::namespace::NamespaceView::new(&self.namespace, self).directory_page_entries(
            directory_key,
            after,
            limit,
        )
    }

    /// Recorded recursive totals `(entries, files)` of one directory (the
    /// empty key is the root). O(directory level), never a tree expansion.
    pub fn directory_totals(&self, directory_key: &str) -> Result<Option<(u64, u64)>> {
        crate::namespace::NamespaceView::new(&self.namespace, self).directory_totals(directory_key)
    }

    /// The first entry carrying `file_id` in canonical order: an early-stop
    /// walk for namespaces without a path hint (file-branch heads are tiny).
    pub fn find_file(&self, file_id: &str) -> Result<Option<Entry>> {
        crate::namespace::NamespaceView::new(&self.namespace, self).find_file(file_id)
    }

    /// Stream every entry in canonical order without materializing the list;
    /// the callback returns whether to continue.
    pub fn walk(&self, mut visit: impl FnMut(&Entry) -> crate::Result<bool>) -> Result<()> {
        crate::namespace::NamespaceView::new(&self.namespace, self).walk(|location, entry| {
            visit(&crate::namespace::materialized_entry(
                location.clone(),
                entry,
            ))
        })
    }

    /// The recorded total entry count of this namespace.
    pub fn total_entries(&self) -> u64 {
        self.namespace.entries
    }
}

impl<'a> NamespaceTree<'a> {
    pub(crate) fn captured(
        namespace: storage_contract::CommitNamespace,
        reader: crate::segment_store::SegmentReader<'a>,
    ) -> Self {
        Self {
            resources: reader.resources,
            caches: reader.caches,
            members: Cow::Borrowed(reader.members),
            observe: reader.observe,
            namespace,
        }
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
                resources: &self.disk_resources,
                caches: &self.segment_caches,
                namespace: commit.namespace.clone(),
                members: Cow::Owned(self.members()),
                observe: None,
            })
    }

    /// The current published namespace with the same admission and errors as
    /// `list_tree`, but with paged normalized-key lookup.
    pub fn namespace_tree(&self, branch_id: &str) -> Result<NamespaceTree<'_>> {
        let namespace = self.tree_commit(branch_id)?.namespace.clone();
        Ok(NamespaceTree {
            resources: &self.disk_resources,
            caches: &self.segment_caches,
            namespace,
            members: Cow::Owned(self.members()),
            observe: None,
        })
    }

    /// A selected published namespace. Foreign commits and migrated branches
    /// retain the `list_tree_at` errors; no per-request tree scan.
    pub fn namespace_tree_at(&self, branch_id: &str, commit_id: &str) -> Result<NamespaceTree<'_>> {
        let namespace = self.tree_commit_at(branch_id, commit_id)?.namespace.clone();
        Ok(NamespaceTree {
            resources: &self.disk_resources,
            caches: &self.segment_caches,
            namespace,
            members: Cow::Owned(self.members()),
            observe: None,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/namespace/query_tests.rs"]
mod tests;
