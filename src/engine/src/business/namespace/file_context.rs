//! The current ordinary namespace's FILE selection. The owner entry, branch and
//! admitted namespace HEAD determine what this reader may expose. Frozen
//! history reads have their own source and must not fall through to this module.
use crate::business::{namespace, repository};
use crate::{Engine, Entry, FileContext, NamespaceEntryKind, Repository, State};

pub(crate) fn context_key_from_key(repository: &str, key: &str) -> String {
    format!("{repository}/{key}")
}
pub(crate) fn context_at_key(state: &State, repository: &str, key: &str) -> Option<FileContext> {
    if let Some(context) = state
        .file_contexts
        .get(&context_key_from_key(repository, key))
    {
        return Some(context.clone());
    }
    let repo = state.repositories.get(repository)?;
    let commit = state
        .commits
        .get(&state.branches.get(&repo.main_branch_id)?.head_commit_id)?;
    let entry = namespace::find_unordered_key(&commit.entries, key)
        .filter(|e| e.kind == NamespaceEntryKind::File)?;
    Some(context_for_entry(state, repo, entry))
}

/// Bootstrap the current FILE context from its stable selection or its owning root.
/// A caller reading a frozen node must use that node's original evidence instead.
pub(in crate::business) fn context_for_entry(
    state: &State,
    repo: &Repository,
    entry: &Entry,
) -> FileContext {
    let branch = state
        .file_selections
        .get(&entry.file_id)
        .cloned()
        .unwrap_or_else(|| repo.main_branch_id.clone());
    FileContext {
        node_id: state
            .branches
            .get(&branch)
            .and_then(|b| b.file_node_id.clone()),
        selection_generation: 0,
        branch_id: branch,
        inherited_from: state.file_write_origins.get(&entry.file_id).cloned(),
    }
}
pub(crate) fn branch_context_at_key(
    state: &State,
    namespace: &str,
    key: &str,
) -> Option<FileContext> {
    let branch = state.branches.get(namespace)?;
    if let Some(repo) = repository::main_for_branch(state, namespace) {
        context_at_key(state, &repo.repository_id, key)
    } else {
        Some(FileContext {
            node_id: branch.file_node_id.clone(),
            selection_generation: 0,
            branch_id: branch.branch_id.clone(),
            inherited_from: None,
        })
    }
}

impl Engine {
    pub fn file_context(&self, repository: &str, path: &str) -> Option<FileContext> {
        let key = name_policy::path_key(path);
        let repo = self.state.repositories.get(repository)?;
        match self.namespace_read_view() {
            crate::engine::NamespaceReadView::AwaitingPublication => return None,
            crate::engine::NamespaceReadView::Published(view) => {
                let branch = self.state.branches.get(&repo.main_branch_id)?;
                if !view.contains_entry(&repo.main_branch_id, &branch.head_commit_id, &key, None) {
                    return None;
                }
            }
            crate::engine::NamespaceReadView::Initial => {}
        }
        context_at_key(&self.state, repository, &key)
    }

    /// Resolve an entry already borrowed from this repository's current main
    /// commit. The caller must not pass an entry from another snapshot. Unlike
    /// `file_context`, this avoids looking the same entry up by path again.
    pub fn file_context_for_current_entry<'a>(
        &'a self,
        repository: &str,
        entry: &Entry,
    ) -> Option<std::borrow::Cow<'a, FileContext>> {
        match self.namespace_read_view() {
            crate::engine::NamespaceReadView::AwaitingPublication => return None,
            crate::engine::NamespaceReadView::Published(view) => {
                let repo = self.state.repositories.get(repository)?;
                let branch = self.state.branches.get(&repo.main_branch_id)?;
                if !view.contains_entry(
                    &repo.main_branch_id,
                    &branch.head_commit_id,
                    entry.lookup_key(),
                    Some(entry),
                ) {
                    return None;
                }
            }
            crate::engine::NamespaceReadView::Initial => {}
        }
        if let Some(context) = self
            .state
            .file_contexts
            .get(&context_key_from_key(repository, entry.lookup_key()))
        {
            return Some(std::borrow::Cow::Borrowed(context));
        }
        let repo = self.state.repositories.get(repository)?;
        (entry.kind == NamespaceEntryKind::File)
            .then(|| std::borrow::Cow::Owned(context_for_entry(&self.state, repo, entry)))
    }

    /// For an object presented as a current FILE, bind the complete ordinary row
    /// to the already admitted visible namespace before reading its selection.
    /// The historical and durable workspace paths retain their original rules.
    pub(crate) fn context_for_visible_file<'a>(
        &'a self,
        repository: &str,
        entry: &Entry,
    ) -> Option<std::borrow::Cow<'a, FileContext>> {
        let repo = self.state.repositories.get(repository)?;
        let actual = self.namespace_tree(&repo.main_branch_id).ok()?.get_key(entry.lookup_key())?;
        if actual.kind != NamespaceEntryKind::File || actual != entry {
            return None;
        }
        self.file_context_for_current_entry(repository, actual)
    }
}
