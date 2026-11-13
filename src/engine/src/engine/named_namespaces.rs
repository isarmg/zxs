//! Publication-bound runtime names over immutable namespace sources.
//! No lookup here authorizes a read or admits a historical occurrence into Current.
use crate::engine::{CurrentNamespaces, Engine, MetadataView, NamespaceReadView};
use crate::{Commit, DirectoryBranch, Entry, Error, Result, State};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, Weak};
use storage_contract::{BranchId, CommitId, DirectoryBranchId, DirectoryTreeId, TrackedMap};

enum BranchRows {
    Frozen(Arc<Commit>),
    Current(Arc<Vec<Entry>>),
}

impl BranchRows {
    fn entries(&self) -> &[Entry] {
        match self {
            Self::Frozen(commit) => &commit.entries,
            Self::Current(entries) => entries,
        }
    }

    fn same_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Frozen(left), Self::Frozen(right)) => Arc::ptr_eq(left, right),
            (Self::Current(left), Self::Current(right)) => Arc::ptr_eq(left, right),
            (Self::Frozen(_), Self::Current(_)) | (Self::Current(_), Self::Frozen(_)) => false,
        }
    }
}

struct BranchScope {
    head: CommitId,
    original: BranchRows,
    projected: OnceLock<NamedRows>,
}

struct DirectoryScope {
    head: DirectoryTreeId,
    current: Option<Arc<Vec<Entry>>>,
    projected: OnceLock<NamedRows>,
}

pub(crate) struct NamedRows {
    entries: crate::NamespaceEntries,
}

impl NamedRows {
    pub(crate) fn entries(&self) -> &[Entry] {
        self.entries.entries()
    }

    pub(crate) fn share(&self) -> crate::NamespaceEntries {
        self.entries.clone()
    }
}

/// Capture is O(number of scope heads), not O(all historical entries). Each
/// scope materializes its names once, on first use. Neither capture nor naming
/// gives authority to bypass the original registry, source or write proofs.
pub(crate) struct NamedNamespaces {
    source: Weak<State>,
    publication: NamespaceReadView,
    locations: TrackedMap<String>,
    names: TrackedMap<String>,
    branches: BTreeMap<BranchId, Arc<BranchScope>>,
    directory_sources: TrackedMap<DirectoryBranch>,
    directories: BTreeMap<DirectoryBranchId, Arc<DirectoryScope>>,
}

fn changed() -> Error {
    crate::persist::path_changes::invalid()
}

impl NamedNamespaces {
    fn capture(
        state: &Arc<State>,
        publication: NamespaceReadView,
        previous: Option<&Self>,
    ) -> Result<Self> {
        let current = publication.admitted()?;
        let same_names = previous.is_some_and(|old| {
            state.file_locations.changes_since(&old.locations).is_empty()
                && state.file_names.changes_since(&old.names).is_empty()
        });
        let branches = state.branches.values().map(|branch| {
            let id = branch.branch_id.parse()?;
            let head = branch.head_commit_id.parse()?;
            let original = match current {
                Some(view) => BranchRows::Current(
                    view.share_branch_entries(&branch.branch_id, &branch.head_commit_id)?,
                ),
                None => BranchRows::Frozen(
                    state.commits.get_shared(&branch.head_commit_id).ok_or_else(changed)?,
                ),
            };
            if same_names
                && let Some(scope) = previous.and_then(|old| old.branches.get(&id))
                && scope.head == head && scope.original.same_source(&original)
            {
                return Ok((id, Arc::clone(scope)));
            }
            Ok((id, Arc::new(BranchScope { head, original, projected: OnceLock::new() })))
        }).collect::<Result<_>>()?;
        let same_directories = previous.is_some_and(|old| {
            state.directory_branches.changes_since(&old.directory_sources).is_empty()
        });
        let directories = state.directory_branches.values().map(|branch| {
            let id = branch.directory_branch_id.parse()?;
            let head = branch.head_tree_id.parse()?;
            let current = current.map(|view| {
                view.share_directory_entries(&branch.directory_branch_id, &branch.head_tree_id)
            }).transpose()?;
            if same_names
                && let Some(scope) = previous.and_then(|old| old.directories.get(&id))
                && scope.head == head
                && match (&scope.current, &current) {
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    (None, None) => same_directories,
                    (Some(_), None) | (None, Some(_)) => false,
                }
            {
                return Ok((id, Arc::clone(scope)));
            }
            Ok((id, Arc::new(DirectoryScope { head, current, projected: OnceLock::new() })))
        }).collect::<Result<_>>()?;
        Ok(Self {
            source: Arc::downgrade(state),
            publication,
            locations: state.file_locations.clone(),
            names: state.file_names.clone(),
            branches,
            directory_sources: state.directory_branches.clone(),
            directories,
        })
    }

    fn require_source(&self, state: &Arc<State>, publication: &NamespaceReadView) -> Result<()> {
        if !self.source.ptr_eq(&Arc::downgrade(state))
            || !self.publication.same_publication(publication)
        {
            return Err(changed());
        }
        Ok(())
    }

    fn rows<'a>(
        &self,
        raw: &[Entry],
        retained: Option<crate::NamespaceEntries>,
        cell: &'a OnceLock<NamedRows>,
    ) -> Result<&'a NamedRows> {
        if let Some(rows) = cell.get() {
            return Ok(rows);
        }
        let names = raw.iter().filter(|entry| entry.kind == crate::NamespaceEntryKind::File)
            .map(|entry| Ok((entry.file_id.parse()?,
                crate::container_storage::container_name_from_locations(
                    &self.locations, &self.names, &entry.file_id,
                )?,
            )))
            .collect::<Result<_>>()?;
        let changes = crate::business::namespace::container_name::plan_file_names(raw, &names)
            .map_err(Error::ContainerName)?;
        let paths: BTreeMap<_, _> = changes.into_iter()
            .map(|change| (change.before, change.after)).collect();
        let entries = match retained {
            Some(retained) if paths.is_empty() => retained,
            Some(_) | None => {
                let mut entries = raw.to_vec();
                for entry in &mut entries {
                    if let Some(path) = paths.get(entry.location()) {
                        entry.relocate(path.clone());
                    }
                }
                crate::business::sorted(&mut entries);
                crate::NamespaceEntries::Projected(Arc::new(entries))
            }
        };
        // Concurrent readers may finish the same deterministic projection. A
        // lost insertion uses the already installed rows of this exact source.
        let _ = cell.set(NamedRows { entries });
        Ok(cell.get().expect("invariant: this scope's rows were installed above"))
    }

    /// Workspace bases may precede the live head. The caller must first admit
    /// the baseline; this owned result does not make a historical source writable.
    /// Only this workspace's rows are visited, never the full commit history.
    pub(crate) fn project_workspace(
        &self,
        source: crate::NamespaceEntries,
    ) -> Result<crate::NamespaceEntries> {
        let cell = OnceLock::new();
        self.rows(source.entries(), Some(source.clone()), &cell)
            .map(NamedRows::share)
    }

    pub(crate) fn branch(&self, branch: BranchId, head: CommitId) -> Result<&NamedRows> {
        let scope = self.branches.get(&branch).ok_or_else(changed)?;
        if scope.head != head {
            return Err(changed());
        }
        let retained = match &scope.original {
            BranchRows::Frozen(commit) => crate::NamespaceEntries::Commit(Arc::clone(commit)),
            BranchRows::Current(entries) => crate::NamespaceEntries::Projected(Arc::clone(entries)),
        };
        self.rows(scope.original.entries(), Some(retained), &scope.projected)
    }

    pub(crate) fn directory(
        &self,
        branch: DirectoryBranchId,
        head: DirectoryTreeId,
    ) -> Result<&NamedRows> {
        let scope = self.directories.get(&branch).ok_or_else(changed)?;
        if scope.head != head {
            return Err(changed());
        }
        let raw = match &scope.current {
            Some(entries) => entries.as_slice(),
            None => &self.directory_sources.get(&branch.to_string()).ok_or_else(changed)?.entries,
        };
        let retained = scope.current.as_ref()
            .map(|entries| crate::NamespaceEntries::Projected(Arc::clone(entries)));
        self.rows(raw, retained, &scope.projected)
    }
}

#[derive(Default)]
pub(crate) struct NameCache {
    current: OnceLock<Arc<NamedNamespaces>>,
    previous: Option<Arc<NamedNamespaces>>,
}

impl NameCache {
    pub(crate) fn seed(&self) -> Self {
        Self {
            current: OnceLock::new(),
            previous: self.current.get().cloned().or_else(|| self.previous.clone()),
        }
    }

    /// Pair installation and invalidation without borrowing unrelated Engine
    /// fields. The owning publication code retains its existing I/O/proof order.
    pub(crate) fn install_state(&mut self, slot: &mut Arc<State>, next: Arc<State>) -> Arc<State> {
        if let Some(current) = self.current.take() {
            self.previous = Some(current);
        }
        std::mem::replace(slot, next)
    }

    pub(crate) fn install_current(
        &mut self,
        slot: &mut Option<Arc<CurrentNamespaces>>,
        next: Option<Arc<CurrentNamespaces>>,
    ) {
        if let Some(current) = self.current.take() {
            self.previous = Some(current);
        }
        *slot = next;
    }

    pub(crate) fn capture(
        &self,
        state: &Arc<State>,
        publication: NamespaceReadView,
    ) -> Result<&Arc<NamedNamespaces>> {
        publication.admitted()?;
        if self.current.get().is_none() {
            let named = Arc::new(NamedNamespaces::capture(
                state,
                publication.clone(),
                self.previous.as_deref(),
            )?);
            let _ = self.current.set(named);
        }
        let named = self.current.get()
            .expect("invariant: the immutable namespace projection was installed above");
        named.require_source(state, &publication)?;
        Ok(named)
    }
}

impl Engine {
    pub(crate) fn named_namespaces(&self) -> Result<&Arc<NamedNamespaces>> {
        self.named_namespaces.capture(&self.state, self.namespace_read_view())
    }
}

impl MetadataView {
    pub(crate) fn named_namespaces(&self) -> Result<&Arc<NamedNamespaces>> {
        self.named_namespaces.capture(&self.state.0, self.current_namespaces.clone())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/named_namespaces_tests.rs"]
mod tests;
