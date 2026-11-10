//! Borrowed queries over a published, canonical namespace. Mutable builders do
//! not implement this interface: appends and renames can temporarily break order.
use super::*;

/// One visible occurrence in a current branch. Identity and display location
/// borrow the same published entry; no historical version scan selects a file.
pub struct CurrentObject<'a> {
    repository: &'a Repository,
    entry: &'a Entry,
    namespace: NamespaceTree<'a>,
}

impl<'a> CurrentObject<'a> {
    pub fn repository(&self) -> &'a Repository {
        self.repository
    }

    pub fn entry(&self) -> &'a Entry {
        self.entry
    }

    /// Nearest initialized directory repository, including the entry itself
    /// when it is a repository root. Ordinary directories inherit their owner.
    pub fn initialized_repository_id(&self) -> Option<&'a str> {
        self.namespace.initialized_repository_id(self.entry.lookup_key())
    }

    /// Inspect only this object's descendant range in the same published view.
    /// Hidden files and deleted directory subtrees do not make it nonempty.
    pub fn has_visible_children(&self) -> bool {
        let prefix = format!("{}/", self.entry.lookup_key());
        let start = self
            .namespace
            .entries
            .partition_point(|entry| entry.lookup_key() < prefix.as_str());
        self.namespace.entries[start..]
            .iter()
            .take_while(|entry| entry.lookup_key().starts_with(&prefix))
            .any(|entry| self.namespace.visible(entry))
    }
}

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

    fn initialized_repository_id(self, key: &str) -> Option<&'a str> {
        let (directories, repository) = self.directories?;
        closest_directory_repository(key, |key| {
            if key.is_empty() {
                return directories.contains_key(repository).then_some(repository);
            }
            self.get_key(key)
                .filter(|entry| {
                    entry.kind == NamespaceEntryKind::Directory
                        && directories.contains_key(&entry.entry_id)
                })
                .map(|entry| entry.entry_id.as_str())
        })
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
    /// Resolve a current branch occurrence. Absence remains a normal result for
    /// create/open callers, and soft-deleted entries stay invisible.
    pub fn locate_object(
        &self,
        branch_id: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<CurrentObject<'_>>> {
        let tree = self.namespace_tree(branch_id)?;
        let Some(entry) = tree.get_key(path.key()) else {
            return Ok(None);
        };
        Ok(Some(CurrentObject {
            repository: self.repository_for_branch(branch_id)?,
            entry,
            namespace: tree,
        }))
    }

    /// Resolve the current main namespace of an outer disk-root repository.
    pub fn locate_current_object(
        &self,
        repository_id: &str,
        path: &name_policy::LookupPath,
    ) -> Result<CurrentObject<'_>> {
        let repository = self.state().repositories.get(repository_id).ok_or_else(|| {
            Error::new(ErrorCode::RepositoryNotFound, "repository missing")
        })?;
        self.locate_object(&repository.main_branch_id, path)?
            .ok_or_else(|| Error::new(ErrorCode::EntryNotFound, "object is absent at this path"))
    }

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
