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
    deletions: Option<&'a storage_contract::TrackedMap<crate::FileDeletionNode>>,
    directories: Option<(
        &'a storage_contract::TrackedMap<storage_contract::directory_git::DirectoryGitRepository>,
        &'a str,
    )>,
}

impl<'a> NamespaceTree<'a> {
    // Only this module constructs views. Engine publication validates canonical
    // form through HistoryManifest::from_state; admitted history uses the same
    // validator. No arbitrary Commit, mutable State or entry slice is accepted
    // by the public or crate-wide API, and queries never repeat that validation.
    fn from_published_entries(entries: &'a [Entry]) -> Self {
        Self {
            entries,
            deletions: None,
            directories: None,
        }
    }

    /// Borrow the canonical order, without allocating or copying entries.
    pub fn entries(self) -> &'a [Entry] {
        self.entries
    }

    /// Read one current visible row without losing its retained namespace index.
    pub fn visible_entry(self, index: usize) -> Option<&'a Entry> {
        self.entries.get(index).filter(|entry| self.visible(entry))
    }

    /// Enumerate the current visible namespace without copying retained rows.
    /// Historical and workspace views have no lifecycle filter.
    pub fn visible_entries(self) -> impl Iterator<Item = &'a Entry> {
        self.entries.iter().filter(move |entry| self.visible(entry))
    }

    fn visible(self, entry: &Entry) -> bool {
        (entry.kind != NamespaceEntryKind::File
            || self
                .deletions
                .is_none_or(|deletions| !deletions.contains_key(&entry.file_id)))
            && self.directories.is_none_or(|(directories, repository)| {
                crate::file_deletion::directory::visibility::visible(
                    directories,
                    repository,
                    self.entries,
                    entry,
                )
            })
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
            .filter(|entry| self.visible(entry))
    }
}

impl crate::NamespaceSnapshot {
    pub fn tree(&self) -> NamespaceTree<'_> {
        NamespaceTree {
            entries: self.entries(),
            deletions: Some(self.file_deletions()),
            directories: Some((self.directory_repositories(), self.repository_id())),
        }
    }
}

impl Engine {
    pub(crate) fn workspace_namespace(&self, workspace: &Workspace) -> Result<NamespaceTree<'_>> {
        self.workspace_namespace_entries(workspace)
            .map(NamespaceTree::from_published_entries)
    }

    /// Borrow the current published namespace with the same admission and
    /// errors as `list_tree_ref`, but with logarithmic normalized-key lookup.
    pub fn namespace_tree(&self, branch_id: &str) -> Result<NamespaceTree<'_>> {
        self.list_tree_ref(branch_id).map(|entries| NamespaceTree {
            entries,
            deletions: Some(&self.state().file_deletions),
            directories: Some((
                &self.state().directory_git_repositories,
                &self.state().histories[&self.state().branches[branch_id].history_tree_id]
                    .repository_id,
            )),
        })
    }

    /// Borrow a selected published namespace. Foreign commits and migrated
    /// branches retain the `list_tree_at_ref` errors; no per-request tree scan.
    pub fn namespace_tree_at(&self, branch_id: &str, commit_id: &str) -> Result<NamespaceTree<'_>> {
        self.list_tree_at_ref(branch_id, commit_id)
            .map(NamespaceTree::from_published_entries)
    }
}

impl MetadataView {
    /// Current listing from the same owned registry projection captured with this
    /// metadata view. Historical business_projection().commits remain unchanged.
    pub fn namespace_tree(&self, branch_id: &str) -> Result<NamespaceTree<'_>> {
        let state = self.business_projection();
        let branch = state
            .branches
            .get(branch_id)
            .ok_or_else(|| Error::new(ErrorCode::BranchNotFound, "branch not found"))?;
        if branch.state == BranchState::Migrated {
            return Err(Error::new(
                ErrorCode::TreeMoved,
                "branch moved to another history",
            ));
        }
        let commit = state
            .commits
            .get(&branch.head_commit_id)
            .ok_or_else(|| Error::new(ErrorCode::StateInvalid, "branch HEAD missing"))?;
        let entries = match self.current_namespaces.admitted()? {
            Some(view) => view.branch_entries(branch_id, &commit.commit_id)?,
            None => &commit.entries,
        };
        Ok(NamespaceTree {
            entries,
            deletions: Some(&state.file_deletions),
            directories: Some((
                &state.directory_git_repositories,
                &state.histories[&branch.history_tree_id].repository_id,
            )),
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/namespace/query_tests.rs"]
mod tests;
