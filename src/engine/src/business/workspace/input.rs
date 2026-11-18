//! Borrow the workspace's logical fields without exposing its upload handles.
//! This view carries neither a lease nor namespace or target-root admission.
use crate::{ContentHash, FileContext, NamespaceOperation, Workspace, WorkspaceState};
use name_policy::PortablePath;

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::Workspace {}
}

/// Only the runtime Workspace implements this logical input. Callers retain the
/// actual workspace authentication, baseline, lease and held-source checks.
pub(crate) trait WorkspaceInput: sealed::Sealed {
    fn workspace_id(&self) -> &str;
    fn branch_id(&self) -> &str;
    fn principal(&self) -> &str;
    fn base_head(&self) -> &str;
    fn fencing_token(&self) -> u64;
    fn state(&self) -> &WorkspaceState;
    fn files(&self) -> impl ExactSizeIterator<Item = FileInput<'_>>;
    fn namespace_operations(&self) -> &[NamespaceOperation];
    fn selected_file_branch(&self, file: &str) -> Option<&str>;
    fn selected_file_context(&self, key: &str) -> Option<&FileContext>;
    fn selected_file_contexts(&self) -> impl Iterator<Item = &FileContext>;
}

/// Borrow existing model fields once; source_path is deliberately unavailable.
#[derive(Clone, Copy)]
pub(crate) struct FileInput<'a> {
    item: &'a str,
    path: &'a PortablePath,
    size: u64,
    hash: &'a ContentHash,
    generation: u64,
}

impl<'a> FileInput<'a> {
    pub(crate) fn item(&self) -> &'a str {
        self.item
    }
    pub(crate) fn path(&self) -> &'a PortablePath {
        self.path
    }
    pub(crate) fn size(&self) -> u64 {
        self.size
    }
    pub(crate) fn hash(&self) -> &'a ContentHash {
        self.hash
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

impl WorkspaceInput for Workspace {
    fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
    fn branch_id(&self) -> &str {
        &self.branch_id
    }
    fn principal(&self) -> &str {
        &self.principal
    }
    fn base_head(&self) -> &str {
        &self.base_head
    }
    fn fencing_token(&self) -> u64 {
        self.fencing_token
    }
    fn state(&self) -> &WorkspaceState {
        &self.state
    }
    fn files(&self) -> impl ExactSizeIterator<Item = FileInput<'_>> {
        self.files.values().map(|file| FileInput {
            item: &file.item_id,
            path: &file.path,
            size: file.size,
            hash: &file.content_hash,
            generation: file.dirty_generation,
        })
    }
    fn namespace_operations(&self) -> &[NamespaceOperation] {
        &self.namespace_operations
    }
    fn selected_file_branch(&self, file: &str) -> Option<&str> {
        self.selected_file_branches.get(file).map(String::as_str)
    }
    fn selected_file_context(&self, key: &str) -> Option<&FileContext> {
        self.selected_file_contexts.get(key)
    }
    fn selected_file_contexts(&self) -> impl Iterator<Item = &FileContext> {
        self.selected_file_contexts.values()
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/workspace_input_tests.rs"]
mod tests;
