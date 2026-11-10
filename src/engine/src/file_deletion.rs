//! File-local soft deletion, exact cancellation, and complete branch-frontier capture.
use crate::{Branch, Engine, Entry, Error, ErrorCode, Repository, Result, State};
use storage_contract::history_window::{
    HistoryFileId, HistoryRepositoryId, HistoryWindowCancellation,
};

pub(crate) mod directory;
pub(crate) mod freeze;
pub(crate) mod lifecycle;
pub(crate) mod occurrence;
pub(crate) mod publication;
pub(crate) mod retained;
pub use lifecycle::{DeleteFileRequest, UndoFileDeletionRequest};
pub(crate) mod batch;
mod head;
pub(crate) mod preflight;

pub use head::{FileDeletionHead, FileDeletionNodeKind};

/// An exact stable file at one currently visible, normalized lookup location.
#[derive(Clone, Copy)]
pub struct FileDeletionTarget<'a> {
    pub repository_id: &'a HistoryRepositoryId,
    pub file_id: &'a HistoryFileId,
    pub path: &'a name_policy::LookupPath,
}

/// A retained branch record and its current head. A merged alias retains its
/// own record and head rather than borrowing the surviving branch's ownership.
#[derive(Debug)]
pub struct FileDeletionBranchHead<'a> {
    branch: &'a Branch,
    head: FileDeletionHead<'a>,
    alias_of: Option<&'a str>,
}
impl<'a> FileDeletionBranchHead<'a> {
    pub fn branch(&self) -> &'a Branch {
        self.branch
    }

    pub fn node_id(&self) -> &str {
        self.head.node_id()
    }

    pub fn head(&self) -> &FileDeletionHead<'a> {
        &self.head
    }

    pub fn alias_of(&self) -> Option<&'a str> {
        self.alias_of
    }
}

/// Complete retained branch and current occurrence inputs for one stable file.
/// Lifetimes bind all records to the same Engine borrow. No body, operation ID,
/// deletion ID or lifecycle state is fabricated, and no serialization is offered.
#[derive(Debug)]
pub struct FileDeletionPreflight<'a> {
    repository: &'a Repository,
    file_id: &'a str,
    sequence: u64,
    branches: Vec<FileDeletionBranchHead<'a>>,
    occurrences: Vec<&'a Entry>,
    parent_node_ids: Vec<String>,
}
impl<'a> FileDeletionPreflight<'a> {
    pub fn repository(&self) -> &'a Repository {
        self.repository
    }

    pub fn file_id(&self) -> &'a str {
        self.file_id
    }

    /// Diagnostic observation sequence only, not an authorization or CAS token.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn branches(&self) -> &[FileDeletionBranchHead<'a>] {
        &self.branches
    }

    pub fn occurrences(&self) -> &[&'a Entry] {
        &self.occurrences
    }

    /// Exact unique current branch HEADs, in deterministic node-ID order.
    pub fn parent_node_ids(&self) -> &[String] {
        &self.parent_node_ids
    }
}

impl Engine {
    /// Observe one disk-root file without changing its state or namespace.
    ///
    /// The caller authenticates before using this trusted local API. The current
    /// namespace uses ordinary repository/history and published-view admission.
    /// Named repositories and files without independent branches remain outside
    /// this entrance. Initialized directory occurrences retain their exact working
    /// composition and are admitted only under the selected writable owners.
    /// A missing path/file, foreign retained branch, cancellation or exceeded
    /// current entry budget is an error; partial branch lists are never returned.
    ///
    /// Retained records are borrowed and auxiliary space is O(A + B + H), for
    /// this file's aliases, branches and distinct heads, each entry-budget bounded.
    /// Namespace and branch tables are scanned once, with cancellation checkpoints.
    /// This does not authorize a later write, keep a content lease or prove durability.
    pub fn file_deletion_preflight(
        &self,
        target: FileDeletionTarget<'_>,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileDeletionPreflight<'_>> {
        preflight::checkpoint(cancellation)?;
        let repository = self
            .state()
            .repositories
            .get(target.repository_id.as_str())
            .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "repository not found"))?;
        self.repository_for_history(&repository.history_tree_id)?;
        let namespace = self.namespace_tree(&repository.main_branch_id)?;
        preflight::require_scope_writable(
            self.state(),
            self.admitted_current_namespaces()?,
            repository,
            namespace.entries(),
            target.file_id.as_str(),
            cancellation,
        )?;
        preflight::capture(
            self.state(),
            repository,
            namespace.entries(),
            target,
            self.config().max_entries,
            cancellation,
        )
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/file_deletion_preflight_tests.rs"]
mod tests;
