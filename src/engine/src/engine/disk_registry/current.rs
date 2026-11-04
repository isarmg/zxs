//! Owned current namespaces. Immutable Commit/DirectoryTree records remain history.
//!
//! Preparation is not activation. Installation consumes the lifecycle verifier's
//! durable publication proof, after the exact D3 State/snapshot has been installed.
//! Recovery must reverify the registry and complete mask against its authenticated
//! source before reconstructing this view. A serialized workspace is never itself
//! a registry admission: workspaces without a live baseline remain fenced.
use super::{DiskRegistryCommitment, Engine};
mod admission;
mod batch;
mod directory_branches;
mod directory_metadata;
mod file_branches;
mod new_scope;
mod relocation;
mod workspace;
use crate::{
    BranchState, Commit, Entry, Error, ErrorCode, NamespaceEntryKind, NamespaceOperation, Result,
    State, Workspace, WorkspacePurpose,
};
use relocation::VerifiedEntryChanges;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A metadata snapshot must distinguish genesis from a failed/pending exit.
#[derive(Clone)]
pub(crate) enum NamespaceReadView {
    Initial,
    AwaitingPublication,
    Published(Arc<CurrentNamespaces>),
}
impl NamespaceReadView {
    pub(crate) fn admitted(&self) -> Result<Option<&CurrentNamespaces>> {
        match self {
            Self::Initial => Ok(None),
            Self::Published(view) => Ok(Some(view)),
            Self::AwaitingPublication => Err(pending_publication()),
        }
    }
}

pub(in crate::engine) fn pending_publication() -> Error {
    Error::new(
        ErrorCode::RecoveryRequired,
        "current disk registry publication has not been admitted",
    )
}

#[derive(Clone)]
pub(in crate::engine) struct CurrentScope {
    pub(in crate::engine::disk_registry) head: String,
    pub(in crate::engine::disk_registry) entries: Arc<Vec<Entry>>,
    pub(in crate::engine::disk_registry) source_head: String,
    pub(in crate::engine::disk_registry) source_entries: Arc<Vec<Entry>>,
    pub(in crate::engine::disk_registry) writable: bool,
    pub(in crate::engine::disk_registry) hidden_paths: BTreeSet<String>,
}

/// Not serializable, with no caller-supplied generation setter.
#[derive(Clone)]
pub(crate) struct CurrentNamespaces {
    pub(in crate::engine::disk_registry) registry: DiskRegistryCommitment,
    pub(in crate::engine::disk_registry) registry_state: Arc<super::VerifiedDiskRegistry>,
    pub(in crate::engine::disk_registry) mask_hash: Option<String>,
    pub(in crate::engine::disk_registry) source_snapshot: String,
    pub(in crate::engine::disk_registry) source_sequence: u64,
    pub(in crate::engine::disk_registry) activation_sequence: u64,
    pub(in crate::engine::disk_registry) source_business_generation: u64,
    pub(in crate::engine::disk_registry) branches: Arc<BTreeMap<String, Arc<CurrentScope>>>,
    pub(in crate::engine::disk_registry) directories: Arc<BTreeMap<String, Arc<CurrentScope>>>,
    pub(in crate::engine::disk_registry) known_entries: Arc<BTreeSet<String>>,
    pub(in crate::engine::disk_registry) known_files: Arc<BTreeSet<String>>,
    pub(in crate::engine::disk_registry) unavailable_versions: Arc<BTreeSet<String>>,
}

pub(in crate::engine) struct PreparedCurrentNamespaces {
    pub(in crate::engine::disk_registry) view: CurrentNamespaces,
}

#[derive(Clone)]
pub(crate) struct WorkspaceNamespaceBaseline {
    registry: DiskRegistryCommitment,
    mask_hash: Option<String>,
    workspace_id: String,
    branch_id: String,
    head: String,
    principal: String,
    fencing_token: u64,
    entries_hash: String,
}

fn stale() -> Error {
    Error::new(
        ErrorCode::WorkspaceChanged,
        "workspace namespace or registry generation changed; staged bytes are preserved",
    )
}
fn unsupported() -> Error {
    Error::new(
        ErrorCode::RecoveryRequired,
        "this namespace import or relocation has not been admitted for the current disk registry",
    )
}

impl CurrentNamespaces {
    pub(in crate::engine) fn registry(&self) -> &DiskRegistryCommitment {
        &self.registry
    }

    pub(in crate::engine) fn activation_sequence(&self) -> u64 {
        self.activation_sequence
    }

    pub(in crate::engine) fn matches_source(&self, state: &State) -> bool {
        self.branches.len() == state.branches.len()
            && self.directories.len() == state.directory_branches.len()
            && self.branches.iter().all(|(id, scope)| {
                state
                    .branches
                    .get(id)
                    .is_some_and(|branch| branch.head_commit_id == scope.head)
            })
            && self.directories.iter().all(|(id, scope)| {
                state
                    .directory_branches
                    .get(id)
                    .is_some_and(|branch| branch.head_tree_id == scope.head)
            })
    }

    pub(crate) fn branch_entries<'a>(&'a self, branch: &str, head: &str) -> Result<&'a [Entry]> {
        let scope = self.branches.get(branch).ok_or_else(stale)?;
        if scope.head != head {
            return Err(stale());
        }
        Ok(&scope.entries)
    }

    pub(crate) fn contains_entry(
        &self,
        branch: &str,
        head: &str,
        key: &str,
        expected: Option<&Entry>,
    ) -> bool {
        self.branches
            .get(branch)
            .filter(|scope| scope.head == head)
            .is_some_and(|scope| {
                scope
                    .entries
                    .binary_search_by(|entry| entry.lookup_key().cmp(key))
                    .is_ok_and(|index| {
                        let entry = &scope.entries[index];
                        entry.kind == NamespaceEntryKind::File
                            && expected.is_none_or(|expected| entry == expected)
                    })
            })
    }

    pub(crate) fn contains_file(&self, branch: &str, head: &str, file: &str) -> bool {
        self.branches.get(branch).is_some_and(|scope| {
            scope.head == head
                && scope
                    .entries
                    .iter()
                    .any(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
        })
    }

    pub(crate) fn directory_entries<'a>(&'a self, branch: &str, head: &str) -> Result<&'a [Entry]> {
        let scope = self.directories.get(branch).ok_or_else(stale)?;
        if scope.head != head {
            return Err(stale());
        }
        Ok(&scope.entries)
    }

    pub(crate) fn require_operation(&self, kind: &str) -> Result<()> {
        // Imports without an exact current occurrence proof remain fenced.
        // Admitted planners must verify their own projected sources and
        // reference closure before a resulting state can be published.
        if kind == "repository.create"
            || kind == "workspace.commit"
            || kind == "workspace.namespace.admit"
            || kind == "file.namespace"
            || kind == "file.metadata"
            || kind == "version.metadata"
            || kind == "file.branch.rename"
            || kind == "file.branch.create"
            || kind == "file.branch.select"
            || kind == "file.default.set"
            || kind == "file.branches.merge"
            || kind == "directory.scope.initialize"
            || kind == "directory.namespace.batch"
            || kind == "directory.branch.rename"
            || kind == "directory.branches.create"
            || kind == "directory.branches.select"
            || kind == "directory.default.select"
            || kind == "directory.default.set"
            || kind == "placement-rule.update"
            || kind == "placement.configure"
            || kind == "protection-policy.update"
            || kind.starts_with("auth.")
        {
            Ok(())
        } else {
            Err(unsupported())
        }
    }

    pub(crate) fn require_write_slot(&self, slot: &str) -> Result<()> {
        if self
            .registry_state
            .registered_definitions()
            .any(|(disk, mode, _)| {
                disk.identity.slot_id == slot && mode == super::RegistrationMode::Enabled
            })
        {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::PlacementInvalid,
                "write route contains a disabled or deregistered storage slot",
            ))
        }
    }

    pub(crate) fn require_namespace_operation(&self, operation: &NamespaceOperation) -> Result<()> {
        match operation {
            NamespaceOperation::Rename { .. }
            | NamespaceOperation::Mkdir { .. }
            | NamespaceOperation::Delete { .. }
            | NamespaceOperation::DeleteTree { .. } => Ok(()),
        }
    }

    pub(crate) fn workspace_baseline(
        &self,
        state: &State,
        workspace: &Workspace,
    ) -> Result<WorkspaceNamespaceBaseline> {
        let scope = self.writable_scope(state, &workspace.branch_id)?;
        if workspace.purpose != WorkspacePurpose::Normal || workspace.base_head != scope.head {
            return Err(stale());
        }
        Ok(WorkspaceNamespaceBaseline {
            registry: self.registry.clone(),
            mask_hash: self.mask_hash.clone(),
            workspace_id: workspace.workspace_id.clone(),
            branch_id: workspace.branch_id.clone(),
            head: workspace.base_head.clone(),
            principal: workspace.principal.clone(),
            fencing_token: workspace.fencing_token,
            entries_hash: crate::object_hash(
                "current-workspace-namespace",
                scope.entries.as_ref(),
            )?,
        })
    }

    pub(crate) fn require_branch_write(&self, state: &State, branch: &str) -> Result<()> {
        self.writable_scope(state, branch).map(|_| ())
    }

    /// Planning inventory only, not a write authorization. Stale view/head is an
    /// error rather than a read-only answer, so it cannot suppress requirements.
    pub(crate) fn branch_writable(&self, state: &State, branch: &str) -> Result<bool> {
        let scope = self.branches.get(branch).ok_or_else(stale)?;
        let record = state.branches.get(branch).ok_or_else(stale)?;
        if scope.head != record.head_commit_id {
            return Err(stale());
        }
        Ok(scope.writable
            && record.state == BranchState::Active
            && record.file_id.as_ref().is_none_or(|file| {
                scope
                    .entries
                    .iter()
                    .any(|entry| entry.kind == NamespaceEntryKind::File && &entry.file_id == file)
            }))
    }

    pub(crate) fn directory_writable(&self, state: &State, branch: &str) -> Result<bool> {
        let scope = self.directories.get(branch).ok_or_else(stale)?;
        let record = state.directory_branches.get(branch).ok_or_else(stale)?;
        if scope.head != record.head_tree_id {
            return Err(stale());
        }
        Ok(scope.writable)
    }

    fn writable_scope(&self, state: &State, branch: &str) -> Result<&CurrentScope> {
        let scope = self.branches.get(branch).ok_or_else(stale)?;
        let record = state.branches.get(branch).ok_or_else(stale)?;
        if !scope.writable || record.state != BranchState::Active {
            return Err(Error::new(
                ErrorCode::BranchNotActive,
                "namespace has no retained writable registration",
            ));
        }
        if scope.head != record.head_commit_id {
            return Err(stale());
        }
        if let Some(file) = &record.file_id
            && !scope
                .entries
                .iter()
                .any(|entry| entry.kind == NamespaceEntryKind::File && &entry.file_id == file)
        {
            return Err(Error::new(
                ErrorCode::FileNotFound,
                "file is absent from the current namespace",
            ));
        }
        Ok(scope)
    }

    pub(crate) fn validate_workspace<'a>(
        &'a self,
        state: &'a State,
        workspace: &Workspace,
        baseline: Option<&WorkspaceNamespaceBaseline>,
    ) -> Result<&'a [Entry]> {
        self.writable_scope(state, &workspace.branch_id)?;
        let baseline = baseline.ok_or_else(stale)?;
        if baseline.registry != self.registry
            || baseline.mask_hash != self.mask_hash
            || baseline.workspace_id != workspace.workspace_id
            || baseline.branch_id != workspace.branch_id
            || baseline.head != workspace.base_head
            || baseline.principal != workspace.principal
            || !(baseline.fencing_token == workspace.fencing_token
                || workspace.state == crate::WorkspaceState::Committed
                    && baseline.fencing_token.checked_add(1) == Some(workspace.fencing_token))
        {
            return Err(stale());
        }
        for operation in &workspace.namespace_operations {
            self.require_namespace_operation(operation)?;
        }
        let scope = &self.branches[&baseline.branch_id];
        if baseline.head == scope.source_head {
            Ok(&scope.source_entries)
        } else {
            Ok(&state.commits.get(&baseline.head).ok_or_else(stale)?.entries)
        }
    }

    /// The transport's item ID is not evidence of a new entry. Reject reusing a
    /// removed historical identity before upload bytes are published.
    pub(crate) fn validate_upload_identity(
        &self,
        entries: &[Entry],
        key: &str,
        item: &str,
    ) -> Result<()> {
        let existing = entries
            .binary_search_by(|entry| entry.lookup_key().cmp(key))
            .ok()
            .map(|index| &entries[index]);
        if self.known_entries.contains(item) && existing.is_none_or(|entry| entry.entry_id != item)
        {
            return Err(Error::new(
                ErrorCode::WorkspaceChanged,
                "upload item reuses an earlier entry identity",
            ));
        }
        Ok(())
    }

    pub(crate) fn removed_paths(&self, branch: &str) -> impl Iterator<Item = &str> {
        self.branches
            .get(branch)
            .filter(|scope| scope.head == scope.source_head)
            .into_iter()
            .flat_map(|scope| scope.hidden_paths.iter().map(String::as_str))
    }

    /// Remove path-local write contexts only. Shared immutable bodies, notes,
    /// history and selections still used by a retained occurrence are preserved.
    pub(crate) fn remove_hidden_contexts(&self, state: &mut State, repository: &str, branch: &str) {
        for path in self.removed_paths(branch) {
            state
                .file_contexts
                .remove(&crate::business::context_key_from_key(repository, path));
        }
    }

    pub(crate) fn validate_draft(&self, branch: &str, entries: &[Entry]) -> Result<()> {
        let scope = self.branches.get(branch).ok_or_else(stale)?;
        self.validate_entries(Some(scope), entries)
    }

    pub(crate) fn validate_workspace_draft(
        &self,
        previous: &State,
        next: &State,
        branch: &str,
        entries: &[Entry],
    ) -> Result<()> {
        let scope = self.branches.get(branch).ok_or_else(stale)?;
        let changes = VerifiedEntryChanges::between_states(&scope.entries, entries, previous, next);
        self.validate_entry_changes(Some(scope), entries, &changes)
    }

    fn validate_entries(&self, scope: Option<&CurrentScope>, entries: &[Entry]) -> Result<()> {
        let changes = VerifiedEntryChanges::between(
            scope.map(|scope| scope.entries.as_slice()).unwrap_or(&[]),
            entries,
        );
        self.validate_entry_changes(scope, entries, &changes)
    }

    fn validate_entry_changes(
        &self,
        scope: Option<&CurrentScope>,
        entries: &[Entry],
        changes: &VerifiedEntryChanges<'_>,
    ) -> Result<()> {
        for entry in entries {
            let original = scope.and_then(|scope| {
                scope
                    .entries
                    .binary_search_by(|old| old.lookup_key().cmp(entry.lookup_key()))
                    .ok()
                    .map(|index| &scope.entries[index])
            });
            let reused = if let Some(original) = original {
                original.entry_id != entry.entry_id
                    || original.kind != entry.kind
                    || original.file_id != entry.file_id
                    || original.path() != entry.path()
            } else {
                let entry_known = self.known_entries.contains(&entry.entry_id);
                let file_known = entry.kind == NamespaceEntryKind::File
                    && self.known_files.contains(&entry.file_id);
                // Helpers must prove their exact owning-namespace projection.
                // A globally retained identity does not authorize a new scope.
                entry_known || file_known
            };
            let removed_path = original
                .is_none_or(|original| original.lookup_key() != entry.lookup_key())
                && scope.is_some_and(|scope| scope.hidden_paths.contains(entry.lookup_key()))
                && (self.known_entries.contains(&entry.entry_id)
                    || entry.kind == NamespaceEntryKind::File
                        && self.known_files.contains(&entry.file_id));
            // A fresh identity may occupy a source path vacated by this exact
            // transition. Historical upload IDs still fail the global fence.
            let fresh_replacement = changes.vacated(entry.lookup_key())
                && !self.known_entries.contains(&entry.entry_id)
                && (entry.kind != NamespaceEntryKind::File
                    || !self.known_files.contains(&entry.file_id));
            if (reused || removed_path)
                && !changes.preserves_occurrence(entry)
                && !fresh_replacement
                || entry.kind == NamespaceEntryKind::File
                    && self.unavailable_versions.contains(&entry.version_id)
            {
                return Err(Error::new(
                    ErrorCode::WorkspaceChanged,
                    "draft imports a hidden occurrence, identity or unavailable version",
                ));
            }
        }
        Ok(())
    }

    /// Derived only before a real publication. Ordinary publication installs the
    /// result atomically with its State; failure leaves the previous view intact.
    pub(in crate::engine) fn advance(&self, previous: &State, next: &State) -> Result<Self> {
        let mut view = self.clone();
        let branch_changes = next.branches.changes_since(&previous.branches);
        let directory_changes = next
            .directory_branches
            .changes_since(&previous.directory_branches);
        if branch_changes.is_empty() && directory_changes.is_empty() {
            return Ok(view);
        }
        let mut changed_branches: Vec<_> = match &branch_changes {
            storage_contract::MapChanges::Keys(keys) => keys
                .iter()
                .filter_map(|id| next.branches.get_key_value(id))
                .collect(),
            storage_contract::MapChanges::All => next.branches.iter().collect(),
        };
        // File-branch synchronization may only borrow an owning namespace that
        // this same publication has already passed through its identity fence.
        changed_branches.sort_by_key(|(_, branch)| branch.file_id.is_some());
        let changed_directories: Vec<_> = match &directory_changes {
            storage_contract::MapChanges::Keys(keys) => keys
                .iter()
                .filter_map(|id| next.directory_branches.get_key_value(id))
                .collect(),
            storage_contract::MapChanges::All => next.directory_branches.iter().collect(),
        };
        for &(id, branch) in &changed_branches {
            if previous
                .branches
                .get(id)
                .is_some_and(|old| old.head_commit_id == branch.head_commit_id)
            {
                continue;
            }
            let commit = next.commits.get(&branch.head_commit_id).ok_or_else(stale)?;
            let old = self.branches.get(id);
            if old.is_some_and(|scope| !scope.writable) {
                return Err(Error::new(
                    ErrorCode::BranchNotActive,
                    "publication changes a namespace with no retained writable registration",
                ));
            }
            let source = self.new_file_branch_source(previous, next, branch, commit)?;
            let selection_source =
                self.directory_selection_source(previous, next, branch, commit)?;
            let validation_scope = selection_source
                .as_ref()
                .or(old.map(Arc::as_ref))
                .or(source);
            let mut changes = VerifiedEntryChanges::between_states(
                validation_scope
                    .map(|scope| scope.entries.as_slice())
                    .unwrap_or(&[]),
                &commit.entries,
                previous,
                next,
            );
            if branch.file_id.is_some() {
                let repository = next
                    .repositories
                    .values()
                    .find(|repo| repo.history_tree_id == branch.history_tree_id)
                    .ok_or_else(stale)?;
                let owner = view
                    .branches
                    .get(&repository.main_branch_id)
                    .ok_or_else(stale)?;
                let initial_owner = old
                    .is_none()
                    .then(|| self.branches.get(&repository.main_branch_id))
                    .flatten();
                changes.synchronize_file_branch(
                    next,
                    branch,
                    &owner.entries,
                    initial_owner.map(Arc::as_ref),
                    &commit.entries,
                )?;
            }
            self.validate_entry_changes(validation_scope, &commit.entries, &changes)?;
            let writable = match old {
                Some(scope) => scope.writable,
                None => self.new_branch_writable(previous, next, branch, commit)?,
            };
            Arc::make_mut(&mut view.branches).insert(
                id.clone(),
                Arc::new(CurrentScope {
                    source_head: old
                        .map(|scope| scope.source_head.clone())
                        .unwrap_or_else(|| branch.head_commit_id.clone()),
                    source_entries: old
                        .map(|scope| Arc::clone(&scope.source_entries))
                        .unwrap_or_else(|| Arc::new(commit.entries.clone())),
                    head: branch.head_commit_id.clone(),
                    entries: Arc::new(commit.entries.clone()),
                    writable,
                    hidden_paths: old
                        .map(|scope| scope.hidden_paths.clone())
                        .unwrap_or_default(),
                }),
            );
        }
        for (id, branch) in changed_directories {
            if let Some(before) = previous.directory_branches.get(id)
                && (before == branch
                    || self.directory_label_preserves_entries(previous, next, before, branch)?)
            {
                continue;
            }
            let old = self.directories.get(id);
            let mut changes = VerifiedEntryChanges::between_states(
                old.map(|scope| scope.entries.as_slice()).unwrap_or(&[]),
                &branch.entries,
                previous,
                next,
            );
            let repository = next
                .repositories
                .get(&branch.repository_id)
                .ok_or_else(stale)?;
            let owner = view
                .branches
                .get(&repository.main_branch_id)
                .ok_or_else(stale)?;
            changes.synchronize_directory(next, branch, &owner.entries)?;
            let fork_source = self.directory_fork_source(previous, next, branch, owner)?;
            self.validate_entry_changes(
                fork_source.as_ref().or(old.map(Arc::as_ref)),
                &branch.entries,
                &changes,
            )?;
            Arc::make_mut(&mut view.directories).insert(
                id.clone(),
                Arc::new(CurrentScope {
                    source_head: old
                        .map(|scope| scope.source_head.clone())
                        .unwrap_or_else(|| branch.head_tree_id.clone()),
                    source_entries: old
                        .map(|scope| Arc::clone(&scope.source_entries))
                        .unwrap_or_else(|| Arc::new(branch.entries.clone())),
                    head: branch.head_tree_id.clone(),
                    entries: Arc::new(branch.entries.clone()),
                    writable: old.map(|scope| scope.writable).unwrap_or(owner.writable),
                    hidden_paths: old
                        .map(|scope| scope.hidden_paths.clone())
                        .unwrap_or_default(),
                }),
            );
        }
        for (id, branch) in changed_branches {
            if previous
                .branches
                .get(id)
                .is_some_and(|old| old.head_commit_id == branch.head_commit_id)
            {
                continue;
            }
            for entry in &next.commits[&branch.head_commit_id].entries {
                if !view.known_entries.contains(&entry.entry_id) {
                    Arc::make_mut(&mut view.known_entries).insert(entry.entry_id.clone());
                }
                if entry.kind == NamespaceEntryKind::File {
                    if !view.known_files.contains(&entry.file_id) {
                        Arc::make_mut(&mut view.known_files).insert(entry.file_id.clone());
                    }
                }
            }
        }
        Ok(view)
    }
}

impl Engine {
    pub(crate) fn namespace_read_view(&self) -> NamespaceReadView {
        match &self.current_namespaces {
            Some(view) => NamespaceReadView::Published(Arc::clone(view)),
            None if self
                .config
                .generation()
                .is_some_and(|generation| generation.portable.exit_registry().is_some()) =>
            {
                NamespaceReadView::AwaitingPublication
            }
            None => NamespaceReadView::Initial,
        }
    }

    pub(crate) fn admitted_current_namespaces(&self) -> Result<Option<&CurrentNamespaces>> {
        match &self.current_namespaces {
            Some(view) => Ok(Some(view)),
            None if self
                .config
                .generation()
                .is_some_and(|generation| generation.portable.exit_registry().is_some()) =>
            {
                Err(pending_publication())
            }
            None => Ok(None),
        }
    }

    pub(in crate::engine) fn current_disk_registry(&self) -> Option<&super::VerifiedDiskRegistry> {
        self.current_namespaces
            .as_ref()
            .map(|view| view.registry_state.as_ref())
    }

    pub(crate) fn current_branch_entries(&self, branch: &str, commit: &Commit) -> Result<&[Entry]> {
        // This method intentionally borrows through self, not an arbitrary commit
        // supplied by a caller. NamespaceTree owns the public historical API.
        let admitted = self
            .state
            .commits
            .get(&commit.commit_id)
            .ok_or_else(stale)?;
        match self.admitted_current_namespaces()? {
            Some(view) => view.branch_entries(branch, &admitted.commit_id),
            None => Ok(&admitted.entries),
        }
    }

    pub(crate) fn workspace_namespace_entries<'a>(
        &'a self,
        workspace: &Workspace,
    ) -> Result<&'a [Entry]> {
        match self.admitted_current_namespaces()? {
            Some(view) => view.validate_workspace(
                &self.state,
                workspace,
                self.workspace_namespace_baselines
                    .get(&workspace.workspace_id),
            ),
            None => self
                .state
                .commits
                .get(&workspace.base_head)
                .map(|commit| commit.entries.as_slice())
                .ok_or_else(stale),
        }
    }

    pub fn current_directory_entries(&self, branch_id: &str) -> Result<&[Entry]> {
        let branch = self
            .state
            .directory_branches
            .get(branch_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DirectoryBranchNotFound,
                    "directory branch not found",
                )
            })?;
        match self.admitted_current_namespaces()? {
            Some(view) => view.directory_entries(branch_id, &branch.head_tree_id),
            None => Ok(&branch.entries),
        }
    }

    pub(in crate::engine) fn install_recovered_namespace_state(
        &mut self,
        next: State,
        snapshot: String,
    ) -> Result<()> {
        let next_view = self
            .current_namespaces
            .as_ref()
            .map(|view| view.advance(&self.state, &next).map(Arc::new))
            .transpose()?;
        self.state = Arc::new(next);
        self.snapshot = Some(snapshot);
        self.current_namespaces = next_view;
        // The lifecycle verifier alone can establish the first active view.
        self.restore_current_disk_exit()?;
        Ok(())
    }
}

impl<'a> crate::engine::PlanningContext<'a> {
    /// The current source for rule resolution. It borrows the admitted planning
    /// input; a frozen Commit supplied by a caller cannot become current data.
    pub(crate) fn current_branch_entries(&self, branch_id: &str) -> Result<&'a [Entry]> {
        let state: &'a State = self.state;
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
        match self.current_namespaces {
            Some(view) => view.branch_entries(branch_id, &branch.head_commit_id),
            None if self
                .config
                .generation()
                .is_some_and(|generation| generation.portable.exit_registry().is_some()) =>
            {
                Err(pending_publication())
            }
            None => state
                .commits
                .get(&branch.head_commit_id)
                .map(|commit| commit.entries.as_slice())
                .ok_or_else(|| Error::new(ErrorCode::StateInvalid, "branch HEAD missing")),
        }
    }
}

impl crate::MetadataView {
    pub fn current_directory_entries(&self, branch_id: &str) -> Result<&[Entry]> {
        let branch = self
            .business_projection()
            .directory_branches
            .get(branch_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DirectoryBranchNotFound,
                    "directory branch not found",
                )
            })?;
        match self.current_namespaces.admitted()? {
            Some(view) => view.directory_entries(branch_id, &branch.head_tree_id),
            None => Ok(&branch.entries),
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/current_namespace_tests.rs"]
mod tests;
