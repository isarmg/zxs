//! Thin immutable namespace ownership for lock-free scans and handle pagination.
//! These values carry data and stale-read witnesses, never serving authority.
use super::{Engine, VerifiedDiskRegistry};
use crate::{BranchState, Commit, Entry, Error, ErrorCode, Result};
use std::sync::{Arc, Weak};

/// Shared entry storage. Constructing this data does not admit it as a namespace.
#[derive(Clone, Debug)]
pub enum NamespaceEntries {
    Commit(Arc<Commit>),
    Projected(Arc<Vec<Entry>>),
}
impl NamespaceEntries {
    pub fn entries(&self) -> &[Entry] {
        match self {
            Self::Commit(commit) => &commit.entries,
            Self::Projected(entries) => entries,
        }
    }
}

/// Shared directory-choice data for scans outside the engine lock. The first
/// capture indexes current scope heads; each visited scope lazily projects names.
/// Frozen sources are shared, without retaining the full State or version table.
/// This is read data only; callers still reauthorize and recheck their scan.
#[derive(Clone)]
pub struct DirectoryNamespaceView {
    branches: storage_contract::TrackedMap<crate::DirectoryBranch>,
    named: Arc<crate::engine::named_namespaces::NamedNamespaces>,
}
impl DirectoryNamespaceView {
    pub fn entries(&self, branch_id: &str) -> Result<&[Entry]> {
        let branch = self.branches.get(branch_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DirectoryBranchNotFound,
                "directory branch not found",
            )
        })?;
        self.named.directory(branch_id.parse()?, branch.head_tree_id.parse()?)
            .map(|rows| rows.entries())
    }
}

/// One current namespace captured without retaining unrelated State/history.
#[derive(Clone)]
pub struct NamespaceSnapshot {
    repository: String,
    directory_repositories:
        storage_contract::TrackedMap<storage_contract::directory_git::DirectoryGitRepository>,
    file_deletions: storage_contract::TrackedMap<crate::FileDeletionNode>,
    branch: String,
    head: String,
    entries: NamespaceEntries,
    registry: Option<Weak<VerifiedDiskRegistry>>,
}
impl NamespaceSnapshot {
    pub(crate) fn repository_id(&self) -> &str {
        &self.repository
    }
    pub(crate) fn directory_repositories(
        &self,
    ) -> &storage_contract::TrackedMap<storage_contract::directory_git::DirectoryGitRepository>
    {
        &self.directory_repositories
    }
    pub(crate) fn file_deletions(&self) -> &storage_contract::TrackedMap<crate::FileDeletionNode> {
        &self.file_deletions
    }
    pub fn entries(&self) -> &[Entry] {
        self.entries.entries()
    }
    pub fn branch_id(&self) -> &str {
        &self.branch
    }
    pub fn head_commit_id(&self) -> &str {
        &self.head
    }
    /// Move only shared read data into a transport-owned handle. The transport
    /// must preserve its captured registry binding and reauthorize every read.
    pub fn into_entries(self) -> NamespaceEntries {
        self.entries
    }
    pub fn witness(&self) -> NamespaceSnapshotWitness {
        NamespaceSnapshotWitness {
            directory_repositories: self.directory_repositories.clone(),
            file_deletions: self.file_deletions.clone(),
            branch: self.branch.clone(),
            head: self.head.clone(),
            entries: match &self.entries {
                NamespaceEntries::Commit(commit) => {
                    NamespaceWitnessEntries::Commit(Arc::downgrade(commit))
                }
                NamespaceEntries::Projected(entries) => {
                    NamespaceWitnessEntries::Projected(Arc::downgrade(entries))
                }
            },
            registry: self.registry.clone(),
        }
    }
}

#[derive(Clone)]
enum NamespaceWitnessEntries {
    Commit(Weak<Commit>),
    Projected(Weak<Vec<Entry>>),
}
/// This witness owns no entries, immutable commit, registry graph or State.
/// It shares only the file-deletion and directory-owner tables so visibility changes also
/// invalidate a detached scan. This is never user authorization.
#[derive(Clone)]
pub struct NamespaceSnapshotWitness {
    directory_repositories:
        storage_contract::TrackedMap<storage_contract::directory_git::DirectoryGitRepository>,
    file_deletions: storage_contract::TrackedMap<crate::FileDeletionNode>,
    branch: String,
    head: String,
    entries: NamespaceWitnessEntries,
    registry: Option<Weak<VerifiedDiskRegistry>>,
}

impl Engine {
    /// Reuse the publication-bound scope index, or construct it on first access.
    /// Directory entries are projected only when the captured view visits them.
    pub fn directory_namespace_view(&self) -> Result<DirectoryNamespaceView> {
        Ok(DirectoryNamespaceView {
            branches: self.state.directory_branches.clone(),
            named: Arc::clone(self.named_namespaces()?),
        })
    }

    /// Capture the same current entries as namespace_tree. A scope copies entries
    /// only on its first access when authoritative names differ from its source.
    /// Errors match current namespace admission and branch/head lookup failures.
    pub fn namespace_snapshot(&self, branch_id: &str) -> Result<NamespaceSnapshot> {
        let branch = self
            .state
            .branches
            .get(branch_id)
            .ok_or_else(|| Error::new(ErrorCode::BranchNotFound, "branch not found"))?;
        if branch.state == BranchState::Migrated {
            return Err(Error::new(
                ErrorCode::TreeMoved,
                "branch moved to another history",
            ));
        }
        let entries = self.named_namespaces()?
            .branch(branch_id.parse()?, branch.head_commit_id.parse()?)?.share();
        let registry = self.admitted_current_namespaces()?
            .map(|view| Arc::downgrade(&view.registry_state));
        Ok(NamespaceSnapshot {
            repository: self.state.histories[&branch.history_tree_id]
                .repository_id
                .clone(),
            directory_repositories: self.state.directory_git_repositories.clone(),
            file_deletions: self.state.file_deletions.clone(),
            branch: branch_id.into(),
            head: branch.head_commit_id.clone(),
            entries,
            registry,
        })
    }

    /// Recheck a detached current scan using head and allocation identities only.
    /// Auth-only publications preserve these identities; registry activation and
    /// namespace changes invalidate them even when raw Commit entries are shared.
    pub fn namespace_snapshot_matches(&self, witness: &NamespaceSnapshotWitness) -> bool {
        let Some(branch) = self.state.branches.get(&witness.branch) else {
            return false;
        };
        if branch.state == BranchState::Migrated
            || branch.head_commit_id != witness.head
            || !self
                .state
                .directory_git_repositories
                .changes_since(&witness.directory_repositories)
                .is_empty()
            || !self
                .state
                .file_deletions
                .changes_since(&witness.file_deletions)
                .is_empty()
        {
            return false;
        }
        let named = match self.named_namespaces() {
            Ok(named) => named,
            Err(_) => return false,
        };
        let rows = match witness.branch.parse().and_then(|id| {
            witness.head.parse().map(|head| (id, head))
        }) {
            Ok((id, head)) => match named.branch(id, head) {
                Ok(rows) => rows.share(),
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        let same_rows = match (&witness.entries, &rows) {
            (NamespaceWitnessEntries::Commit(expected), NamespaceEntries::Commit(actual)) => {
                expected.ptr_eq(&Arc::downgrade(actual))
            }
            (NamespaceWitnessEntries::Projected(expected), NamespaceEntries::Projected(actual)) => {
                expected.ptr_eq(&Arc::downgrade(actual))
            }
            (NamespaceWitnessEntries::Commit(_), NamespaceEntries::Projected(_))
            | (NamespaceWitnessEntries::Projected(_), NamespaceEntries::Commit(_)) => false,
        };
        same_rows && match (self.namespace_read_view(), &witness.registry) {
            (super::current::NamespaceReadView::Initial, None) => true,
            (super::current::NamespaceReadView::Published(view), Some(registry)) => {
                registry.ptr_eq(&Arc::downgrade(&view.registry_state))
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/namespace_snapshot_tests.rs"]
mod tests;
