//! Thin immutable namespace ownership for lock-free scans and handle pagination.
//! These values carry data and stale-read witnesses, never serving authority.
use super::{Engine, VerifiedDiskRegistry};
use crate::{BranchState, Commit, Entry, Error, ErrorCode, Result};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};

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

/// An O(1) capture for directory-choice scans outside the engine lock. The
/// shared tables exclude State, commits, versions and unrelated namespace data.
/// This is read data only; callers still reauthorize and recheck their scan.
#[derive(Clone)]
pub struct DirectoryNamespaceView {
    branches: storage_contract::TrackedMap<crate::DirectoryBranch>,
    projected: Option<Arc<BTreeMap<String, Arc<super::current::CurrentScope>>>>,
}
impl DirectoryNamespaceView {
    pub fn entries(&self, branch_id: &str) -> Result<&[Entry]> {
        let branch = self.branches.get(branch_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DirectoryBranchNotFound,
                "directory branch not found",
            )
        })?;
        match &self.projected {
            Some(scopes) => scopes
                .get(branch_id)
                .filter(|scope| scope.head == branch.head_tree_id)
                .map(|scope| scope.entries.as_slice())
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::WorkspaceChanged,
                        "current directory namespace head differs",
                    )
                }),
            None => Ok(&branch.entries),
        }
    }
}

/// One current namespace captured without retaining unrelated State/history.
#[derive(Clone)]
pub struct NamespaceSnapshot {
    branch: String,
    head: String,
    entries: NamespaceEntries,
    registry: Option<Weak<VerifiedDiskRegistry>>,
}
impl NamespaceSnapshot {
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
/// It checks a current scan's identity only, never user authorization.
#[derive(Clone)]
pub struct NamespaceSnapshotWitness {
    branch: String,
    head: String,
    entries: NamespaceWitnessEntries,
    registry: Option<Weak<VerifiedDiskRegistry>>,
}

impl Engine {
    /// Capture shared directory metadata and its current projection without
    /// enumerating branches or copying entries while the engine is borrowed.
    pub fn directory_namespace_view(&self) -> Result<DirectoryNamespaceView> {
        let current = self.admitted_current_namespaces()?;
        Ok(DirectoryNamespaceView {
            branches: self.state.directory_branches.clone(),
            projected: current.map(|view| Arc::clone(&view.directories)),
        })
    }

    /// Capture the same current entries as namespace_tree, without copying them.
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
        let (entries, registry) = match self.admitted_current_namespaces()? {
            Some(view) => {
                let scope = view
                    .branches
                    .get(branch_id)
                    .filter(|scope| scope.head == branch.head_commit_id)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::WorkspaceChanged,
                            "current namespace head differs",
                        )
                    })?;
                (
                    NamespaceEntries::Projected(Arc::clone(&scope.entries)),
                    Some(Arc::downgrade(&view.registry_state)),
                )
            }
            None => (
                NamespaceEntries::Commit(
                    self.state
                        .commits
                        .get_shared(&branch.head_commit_id)
                        .ok_or_else(|| {
                            Error::new(ErrorCode::StateInvalid, "branch HEAD missing")
                        })?,
                ),
                None,
            ),
        };
        Ok(NamespaceSnapshot {
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
        if branch.state == BranchState::Migrated || branch.head_commit_id != witness.head {
            return false;
        }
        match (
            self.namespace_read_view(),
            &witness.entries,
            &witness.registry,
        ) {
            (
                super::current::NamespaceReadView::Initial,
                NamespaceWitnessEntries::Commit(expected),
                None,
            ) => self
                .state
                .commits
                .get_shared(&witness.head)
                .is_some_and(|commit| expected.ptr_eq(&Arc::downgrade(&commit))),
            (
                super::current::NamespaceReadView::Published(view),
                NamespaceWitnessEntries::Projected(expected),
                Some(registry),
            ) => {
                registry.ptr_eq(&Arc::downgrade(&view.registry_state))
                    && view.branches.get(&witness.branch).is_some_and(|scope| {
                        scope.head == witness.head
                            && expected.ptr_eq(&Arc::downgrade(&scope.entries))
                    })
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/namespace_snapshot_tests.rs"]
mod tests;
