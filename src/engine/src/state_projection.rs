//! Shared State-derived membership and exact projection closure over map writes.
//!
//! The owner index is derived only from State. Its publication lifetime is bound
//! by PublishedStateBase, never by SQLite, sequence equality or a graph hash.
use crate::{Result, State};
use json_codec::Value;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{ContentState, MapChanges, NamespaceEntryKind, SharedRecords};
mod files;
pub(crate) mod lifecycles;
mod move_receipts;
pub(crate) use files::FileRows;

// One schema drives State coverage, row lookup, membership, and graph field names.
macro_rules! projection_maps {
    ($consumer:ident) => { $consumer! {
        container_edits => ContainerEdits, file, "container_edits";
        container_tags => ContainerTags, file, "container_tags";
        branches => Branches, history, "branches";
        file_selections => FileSelections, selected_file, "file_selections";
        directory_branches => DirectoryBranches, repository, "directory_branches";
        directory_git_repositories => DirectoryGitRepositories, repository, "directory_git_repositories";
        directory_trees => DirectoryTrees, directory_tree, "directory_trees";
        file_write_origins => FileWriteOrigins, file, "file_write_origins";
        file_contexts => FileContexts, context, "file_contexts";
        file_path_nodes => FilePathNodes, history, "file_path_nodes";
        file_metadata_nodes => FileMetadataNodes, history, "file_metadata_nodes";
        file_deletions => FileDeletions, history, "file_deletions";
        file_branch_merges => FileBranchMerges, history, "file_branch_merges";
        file_branch_aliases => FileBranchAliases, alias, "file_branch_aliases";
        file_node_parents => FileNodeParents, version, "file_node_parents";
        file_path_notes => FilePathNotes, node_note, "file_path_notes";
        directory_tree_selections => DirectoryTreeSelections, directory, "directory_tree_selections";
        file_locations => FileLocations, file, "file_locations";
        file_backup_paths => FileBackupPaths, file, "file_backup_paths";
        file_backup_policies => FileBackupPolicies, file, "file_backup_policies";
        directory_selections => DirectorySelections, selected_directory, "directory_selections";
        directory_selection_generations => DirectorySelectionGenerations, directory, "directory_selection_generations";
        file_selection_generations => FileSelectionGenerations, file, "file_selection_generations";
        file_notes => FileNotes, file, "file_notes";
        version_notes => VersionNotes, version, "version_notes";
        file_names => FileNames, file, "file_names";
        commits => Commits, history, "commits";
        versions => Versions, history, "versions";
        placements => Placements, version, "placements";
        merge_links => MergeLinks, history, "merge_links";
        history_boundaries => BoundaryMarks, history, "boundary_marks";
        content_states => ContentStates, version, "content_states";
        placement_rules => PlacementRules, repository, "placement_rules";
        protection_policies => ProtectionPolicies, repository, "protection_policies";
    }};
}

macro_rules! define_fields {
    ($($field:ident => $variant:ident, $owner:ident, $name:literal;)*) => {
        #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
        pub(super) enum Field { $($variant,)* PathAudit }
        impl Field {
            pub(super) const ALL: &'static [Self] = &[$(Self::$variant,)* Self::PathAudit];
            pub(super) fn name(self) -> &'static str {
                match self { $(Self::$variant => $name,)* Self::PathAudit => "path_audit", }
            }
            fn keys(self, state: &State, mut visit: impl FnMut(&str)) {
                match self {
                    $(Self::$variant => field_keys!($field, state, visit),)*
                    Self::PathAudit => {
                        for key in state.operations.keys() {
                            visit(&key.to_string());
                        }
                    }
                }
            }
            pub(super) fn value(self, state: &State, key: &str) -> Result<Option<Value>> {
                match self {
                    $(Self::$variant => field_value!($field, state, key),)*
                    Self::PathAudit => {
                        let id = key.parse::<storage_contract::OperationId>()?;
                        if !state.operations.contains_key(&id) {
                            return Ok(None);
                        }
                        Ok(crate::history_package::path_audit::carrier(state, id)?
                            .map(json_codec::to_value).transpose()?)
                    }
                }
            }
            fn equal(self, before: &State, after: &State, key: &str) -> bool {
                match self {
                    $(Self::$variant => field_equal!($field, before, after, key),)*
                    Self::PathAudit => key.parse::<storage_contract::OperationId>()
                        .is_ok_and(|id| before.operations.get(&id) == after.operations.get(&id)
                            && before.operation_principals.get(&id)
                                == after.operation_principals.get(&id)),
                }
            }
            fn owners(self, state: &State, index: &DerivedOwners, key: &str) -> BTreeSet<String> {
                if !self.present(state, key) { return BTreeSet::new(); }
                match self {
                    $(Self::$variant => owner_for!($owner, state, index, key, $field),)*
                    Self::PathAudit => index.path_audit_histories(state, key),
                }
            }
            fn present(self, state: &State, key: &str) -> bool {
                match self {
                    $(Self::$variant => field_present!($field, state, key),)*
                    Self::PathAudit => key.parse::<storage_contract::OperationId>()
                        .is_ok_and(|id| state.operations.contains_key(&id)),
                }
            }
        }
        impl Writes {
            fn between(before: &State, after: &State) -> Option<Self> {
                let State {
                    sequence: _, business_generation: _, topology_generation: _,
                    control_auth: _, events: _, operations: _, operation_principals: _, plans: _,
                    histories: _, repositories: _, $($field: _,)*
                } = after;
                let mut writes = Self {
                    histories: exact_keys(after.histories.changes_since(&before.histories))?,
                    repositories: exact_keys(after.repositories.changes_since(&before.repositories))?,
                    operations: exact_keys(after.operations.changes_since(&before.operations))?
                        .union(&exact_keys(after.operation_principals.changes_since(&before.operation_principals))?)
                        .copied().collect(),
                    ..Self::default()
                };
                $(writes.rows.insert(Field::$variant,
                    exact_keys(after.$field.changes_since(&before.$field))?
                        .into_iter().map(|key| key.to_string()).collect());)*
                writes.rows.insert(Field::PathAudit,
                    writes.operations.iter().map(ToString::to_string).collect());
                Some(writes)
            }
        }
    };
}

macro_rules! owner_for {
    (history, $state:ident, $index:ident, $key:ident, $field:ident) => {
        one(Some($state.$field[$key].history_tree_id.as_str()))
    };
    (repository, $state:ident, $index:ident, $key:ident, $field:ident) => {
        $index
            .repositories
            .owners(&$state.$field[$key].repository_id)
    };
    (version, $state:ident, $index:ident, $key:ident, $field:ident) => {
        one($state
            .versions
            .get($key)
            .map(|version| version.history_tree_id.as_str()))
    };
    (file, $state:ident, $index:ident, $key:ident, $field:ident) => {
        $index.file_histories($key).cloned().collect()
    };
    (directory, $state:ident, $index:ident, $key:ident, $field:ident) => {
        $index.directories.owners($key)
    };
    (context, $state:ident, $index:ident, $key:ident, $field:ident) => {
        $key.split_once('/')
            .map(|(repository, _)| $index.repositories.owners(repository))
            .unwrap_or_default()
    };
    (alias, $state:ident, $index:ident, $key:ident, $field:ident) => {
        one($state
            .branches
            .get($key)
            .map(|branch| branch.history_tree_id.as_str()))
    };
    (selected_file, $state:ident, $index:ident, $key:ident, $field:ident) => {
        one($state
            .file_selections
            .get($key)
            .and_then(|branch| $state.branches.get(branch))
            .map(|branch| branch.history_tree_id.as_str()))
    };
    (directory_tree, $state:ident, $index:ident, $key:ident, $field:ident) => {
        directory_branch_owners(
            $state,
            $index,
            $state
                .directory_trees
                .get($key)
                .map(|tree| tree.directory_branch_id.as_str()),
        )
    };
    (selected_directory, $state:ident, $index:ident, $key:ident, $field:ident) => {
        directory_branch_owners(
            $state,
            $index,
            $state.directory_selections.get($key).map(String::as_str),
        )
    };
    (node_note, $state:ident, $index:ident, $key:ident, $field:ident) => {
        $state
            .file_path_nodes
            .get($key)
            .map(|node| node.history_tree_id.clone())
            .into_iter()
            .chain(
                $state
                    .file_metadata_nodes
                    .get($key)
                    .map(|node| node.history_tree_id.clone()),
            )
            .collect()
    };
}
macro_rules! field_keys {
    (container_edits, $state:ident, $visit:ident) => {
        for key in $state.container_edits.keys() {
            $visit(&key.to_string());
        }
    };
    (container_tags, $state:ident, $visit:ident) => {
        for key in $state.container_tags.keys() {
            $visit(&key.to_string());
        }
    };

    (content_states, $state:ident, $visit:ident) => {
        for key in $state.versions.keys() {
            $visit(key);
        }
    };
    ($field:ident, $state:ident, $visit:ident) => {
        for key in $state.$field.keys() {
            $visit(key);
        }
    };
}
macro_rules! field_value {
    (container_edits, $state:ident, $key:ident) => {
        Ok($key.parse::<storage_contract::FileId>().ok()
            .and_then(|key| $state.container_edits.get(&key))
            .map(json_codec::to_value).transpose()?)
    };
    (container_tags, $state:ident, $key:ident) => {
        Ok($key.parse::<storage_contract::FileId>().ok()
            .and_then(|key| $state.container_tags.get(&key))
            .map(json_codec::to_value).transpose()?)
    };

    (content_states, $state:ident, $key:ident) => {
        Ok($state
            .versions
            .get($key)
            .map(|_| json_codec::to_value(content_state($state, $key)))
            .transpose()?)
    };
    (directory_selection_generations, $state:ident, $key:ident) => {
        Ok($state
            .directory_selection_generations
            .get($key)
            .map(|value| Value::String(value.to_string())))
    };
    (file_selection_generations, $state:ident, $key:ident) => {
        Ok($state
            .file_selection_generations
            .get($key)
            .map(|value| Value::String(value.to_string())))
    };
    ($field:ident, $state:ident, $key:ident) => {
        Ok($state
            .$field
            .get($key)
            .map(json_codec::to_value)
            .transpose()?)
    };
}
macro_rules! field_equal {
    (container_edits, $before:ident, $after:ident, $key:ident) => {
        $key.parse::<storage_contract::FileId>().ok().is_some_and(|key| {
            $before.container_edits.get(&key) == $after.container_edits.get(&key)
        })
    };
    (container_tags, $before:ident, $after:ident, $key:ident) => {
        $key.parse::<storage_contract::FileId>().ok().is_some_and(|key| {
            $before.container_tags.get(&key) == $after.container_tags.get(&key)
        })
    };

    (content_states, $before:ident, $after:ident, $key:ident) => {
        $before.versions.contains_key($key) == $after.versions.contains_key($key)
            && content_state($before, $key) == content_state($after, $key)
    };
    ($field:ident, $before:ident, $after:ident, $key:ident) => {
        $before.$field.get($key) == $after.$field.get($key)
    };
}
macro_rules! field_present {
    (container_edits, $state:ident, $key:ident) => {
        $key.parse::<storage_contract::FileId>().ok()
            .is_some_and(|key| $state.container_edits.contains_key(&key))
    };
    (container_tags, $state:ident, $key:ident) => {
        $key.parse::<storage_contract::FileId>().ok()
            .is_some_and(|key| $state.container_tags.contains_key(&key))
    };

    (content_states, $state:ident, $key:ident) => {
        $state.versions.contains_key($key)
    };
    ($field:ident, $state:ident, $key:ident) => {
        $state.$field.contains_key($key)
    };
}
projection_maps!(define_fields);

fn content_state(state: &State, key: &str) -> ContentState {
    // Missing state is the existing logical Available representation.
    state
        .content_states
        .get(key)
        .copied()
        .unwrap_or(ContentState::Available)
}
fn one(owner: Option<&str>) -> BTreeSet<String> {
    owner.into_iter().map(str::to_owned).collect()
}
fn directory_branch_owners(
    state: &State,
    index: &DerivedOwners,
    branch: Option<&str>,
) -> BTreeSet<String> {
    branch
        .and_then(|id| state.directory_branches.get(id))
        .map(|branch| index.repositories.owners(&branch.repository_id))
        .unwrap_or_default()
}
fn exact_keys<K: Ord + Clone>(changes: MapChanges<K>) -> Option<BTreeSet<K>> {
    match changes {
        MapChanges::Keys(keys) => Some(keys.as_ref().clone()),
        MapChanges::All => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct References(SharedRecords<SharedRecords<usize>>);
impl References {
    fn add(&mut self, key: &str, owner: &str) -> bool {
        if !self.0.contains_key(key) {
            self.0.insert(key.to_owned(), SharedRecords::new());
        }
        let owners = self
            .0
            .get_mut(key)
            .expect("invariant: reference group was inserted");
        if let Some(count) = owners.get_mut(owner) {
            *count += 1;
            false
        } else {
            owners.insert(owner.to_owned(), 1);
            true
        }
    }
    fn remove(&mut self, key: &str, owner: &str) -> bool {
        let owners = self
            .0
            .get_mut(key)
            .expect("invariant: old State contributed this reference group");
        let count = owners
            .get_mut(owner)
            .expect("invariant: old State contributed this owner reference");
        *count -= 1;
        let removed = *count == 0;
        if removed {
            owners.remove_discard(owner);
        }
        if owners.is_empty() {
            self.0.remove_discard(key);
        }
        removed
    }
    fn owners(&self, key: &str) -> BTreeSet<String> {
        self.0
            .get(key)
            .into_iter()
            .flat_map(SharedRecords::keys)
            .cloned()
            .collect()
    }
    fn each(&self, key: &str, mut visit: impl FnMut(&str)) {
        if let Some(owners) = self.0.get(key) {
            for owner in owners.keys() {
                visit(owner);
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Rows(BTreeMap<Field, SharedRecords<()>>);
impl Rows {
    fn insert(&mut self, field: Field, key: &str) {
        self.0.entry(field).or_default().insert(key.to_owned(), ());
    }
    fn remove(&mut self, field: Field, key: &str) {
        let rows = self
            .0
            .get_mut(&field)
            .expect("invariant: indexed row has its field");
        assert!(
            rows.remove_discard(key),
            "invariant: old owner contains its projected row"
        );
        if rows.is_empty() {
            self.0.remove(&field);
        }
    }
    pub(super) fn values(
        &self,
        field: Field,
        state: &State,
    ) -> Result<json_codec::Map<String, Value>> {
        self.0
            .get(&field)
            .into_iter()
            .flat_map(SharedRecords::keys)
            .map(|key| {
                Ok((
                    key.clone(),
                    field
                        .value(state, key)?
                        .expect("invariant: indexed projection row exists"),
                ))
            })
            .collect()
    }
    fn dirty(&self, writes: &mut Writes) {
        for (&field, rows) in &self.0 {
            for key in rows.keys() {
                writes.row(field, key);
            }
        }
    }
}

/// Exact file/history/version membership, shared by both projections.
#[derive(Clone, Debug, Default, PartialEq)]
struct FileVersions(SharedRecords<SharedRecords<SharedRecords<()>>>);

/// Rebuildable dependency counts and projected row membership; no business data.
/// All unbounded containers use the same SharedRecords API as State records.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DerivedOwners {
    repositories: References,
    files: FileVersions,
    directories: References,
    selected_files: References,
    selected_directories: References,
    directory_trees: References,
    repository_rows: SharedRecords<Rows>,
    history_rows: SharedRecords<Rows>,
    file_rows: SharedRecords<FileRows>,
    branch_contexts: SharedRecords<SharedRecords<()>>,
    branch_links: References,
    unscoped_deletions: lifecycles::DeletionOperations,
    unscoped_history_copies: lifecycles::DeletionOperations,
    unscoped_path_changes: lifecycles::DeletionOperations,
    path_audit_owners: SharedRecords<lifecycles::AuditOwner, storage_contract::OperationId>,
    unscoped_path_audit: lifecycles::DeletionOperations,
}

#[derive(Clone, Default)]
struct Writes {
    operations: BTreeSet<storage_contract::OperationId>,
    rows: BTreeMap<Field, BTreeSet<String>>,
    histories: BTreeSet<String>,
    repositories: BTreeSet<String>,
}
impl Writes {
    fn row(&mut self, field: Field, key: &str) {
        self.rows.entry(field).or_default().insert(key.to_owned());
    }
    fn file(&mut self, key: &str) {
        for field in [
            Field::ContainerTags,
            Field::ContainerEdits,
            Field::FileWriteOrigins,
            Field::FileLocations,
            Field::FileBackupPaths,
            Field::FileBackupPolicies,
            Field::FileSelectionGenerations,
            Field::FileNotes,
            Field::FileNames,
        ] {
            self.row(field, key);
        }
    }
    fn directory(&mut self, key: &str) {
        self.row(Field::DirectoryTreeSelections, key);
        self.row(Field::DirectorySelectionGenerations, key);
    }
    fn version(&mut self, key: &str) {
        for field in [
            Field::Placements,
            Field::ContentStates,
            Field::FileNodeParents,
            Field::VersionNotes,
        ] {
            self.row(field, key);
        }
    }
}

#[derive(Default)]
pub(super) struct HistoryEdits {
    pub(super) maps: BTreeMap<Field, BTreeMap<String, Option<Value>>>,
    pub(super) repository: bool,
    pub(super) history: bool,
}
pub(super) struct ProjectionDelta {
    pub(super) owners: DerivedOwners,
    pub(super) histories: BTreeMap<String, HistoryEdits>,
}

/// One immutable candidate transition supplies both history and physical-name
/// projections. Only the publication owner may retain `next` after success.
pub(crate) struct ProjectionTransition<'a> {
    before: &'a State,
    after: &'a State,
    previous: std::borrow::Cow<'a, DerivedOwners>,
    next: DerivedOwners,
    history_edits: Option<BTreeMap<String, HistoryEdits>>,
    previous_moves: move_receipts::MoveReceipts<'a>,
    next_moves: move_receipts::MoveReceipts<'a>,
}

#[derive(Clone, Copy)]
pub(crate) struct StateProjection<'a> {
    state: &'a State,
    owners: &'a DerivedOwners,
    moves: &'a move_receipts::MoveReceipts<'a>,
}

/// A call-local, read-only projection of one immutable State. It derives
/// membership and decoded facts, not publication or physical-disk authority.
pub(crate) struct ProjectionSnapshot<'a> {
    state: &'a State,
    owners: DerivedOwners,
    moves: move_receipts::MoveReceipts<'a>,
}

impl<'a> ProjectionSnapshot<'a> {
    pub(crate) fn new(state: &'a State) -> Self {
        Self {
            state,
            owners: DerivedOwners::from_state(state),
            moves: move_receipts::MoveReceipts::new(state),
        }
    }

    pub(crate) fn view(&self) -> StateProjection<'_> {
        StateProjection {
            state: self.state,
            owners: &self.owners,
            moves: &self.moves,
        }
    }
}

impl<'a> StateProjection<'a> {
    pub(crate) fn state(self) -> &'a State {
        self.state
    }
    pub(crate) fn owners(self) -> &'a DerivedOwners {
        self.owners
    }

    pub(crate) fn object_move(
        self,
        id: storage_contract::OperationId,
    ) -> Result<std::sync::Arc<move_receipts::MoveReceipt>> {
        self.moves.get(id)
    }
}

impl<'a> ProjectionTransition<'a> {
    pub(crate) fn new(
        before: &'a State,
        after: &'a State,
        baseline: Option<&'a DerivedOwners>,
    ) -> Result<Self> {
        let previous = match baseline {
            Some(index) => std::borrow::Cow::Borrowed(index),
            None => std::borrow::Cow::Owned(DerivedOwners::from_state(before)),
        };
        let (next, history_edits) = match baseline {
            Some(index) => match ProjectionDelta::between(before, after, index)? {
                Some(delta) => (delta.owners, Some(delta.histories)),
                None => (DerivedOwners::from_state(after), None),
            },
            None => {
                // The freshly derived previous index proves pure membership,
                // not a published history graph. Keep full history admission.
                let next = DerivedOwners::transition(before, after, &previous, None)?
                    .unwrap_or_else(|| DerivedOwners::from_state(after));
                (next, None)
            }
        };
        Ok(Self {
            before,
            after,
            previous,
            next,
            history_edits,
            previous_moves: move_receipts::MoveReceipts::new(before),
            next_moves: move_receipts::MoveReceipts::new(after),
        })
    }

    pub(crate) fn previous(&self) -> StateProjection<'_> {
        StateProjection {
            state: self.before,
            owners: &self.previous,
            moves: &self.previous_moves,
        }
    }

    pub(crate) fn next(&self) -> StateProjection<'_> {
        StateProjection {
            state: self.after,
            owners: &self.next,
            moves: &self.next_moves,
        }
    }

    pub(crate) fn take_history_edits(&mut self) -> Option<BTreeMap<String, HistoryEdits>> {
        self.history_edits.take()
    }

    pub(crate) fn into_owners(self) -> DerivedOwners {
        self.next
    }
}

impl DerivedOwners {
    pub(crate) fn from_state(state: &State) -> Self {
        let mut index = Self::default();
        for history in state.histories.values() {
            index
                .repositories
                .add(&history.repository_id, &history.history_tree_id);
            index
                .directories
                .add(&history.repository_id, &history.history_tree_id);
        }
        for (key, version) in state.versions.iter() {
            index
                .files
                .add(&version.file_id, &version.history_tree_id, key);
        }
        for commit in state.commits.values() {
            index.commit_directories(commit, true);
        }
        for (key, branch) in state.file_selections.iter() {
            index.selected_files.add(branch, key);
        }
        for (key, branch) in state.directory_selections.iter() {
            index.selected_directories.add(branch, key);
        }
        for (key, tree) in state.directory_trees.iter() {
            index.directory_trees.add(&tree.directory_branch_id, key);
        }
        for field in [
            Field::DirectoryBranches,
            Field::PlacementRules,
            Field::ProtectionPolicies,
            Field::FileContexts,
        ] {
            field.keys(state, |key| index.repository_row(state, field, key, true));
        }
        index.build_file_rows(state);
        index.build_deletion_rows(state);
        for &field in Field::ALL {
            field.keys(state, |key| {
                for history in field.owners(state, &index, key) {
                    index.member(&history, field, key, true);
                }
            });
        }
        index
    }
    pub(super) fn rows(&self, history: &str) -> Option<&Rows> {
        self.history_rows.get(history)
    }
    fn member(&mut self, history: &str, field: Field, key: &str, add: bool) {
        if add {
            if !self.history_rows.contains_key(history) {
                self.history_rows
                    .insert(history.to_owned(), Rows::default());
            }
            self.history_rows
                .get_mut(history)
                .expect("invariant: history row group exists")
                .insert(field, key);
        } else {
            let rows = self
                .history_rows
                .get_mut(history)
                .expect("invariant: old row has a history group");
            rows.remove(field, key);
            if rows.0.is_empty() {
                self.history_rows.remove_discard(history);
            }
        }
    }
    fn commit_directories(&mut self, commit: &storage_contract::Commit, add: bool) {
        for entry in &commit.entries {
            if entry.kind == NamespaceEntryKind::Directory {
                if add {
                    self.directories
                        .add(&entry.entry_id, &commit.history_tree_id);
                } else {
                    self.directories
                        .remove(&entry.entry_id, &commit.history_tree_id);
                }
            }
        }
    }
    fn repository_row(&mut self, state: &State, field: Field, key: &str, add: bool) {
        let repository = repository_key(state, field, key);
        let Some(repository) = repository else {
            return;
        };
        if add {
            if !self.repository_rows.contains_key(repository) {
                self.repository_rows
                    .insert(repository.to_owned(), Rows::default());
            }
            self.repository_rows
                .get_mut(repository)
                .expect("invariant: repository row group exists")
                .insert(field, key);
        } else {
            let rows = self
                .repository_rows
                .get_mut(repository)
                .expect("invariant: old repository row group exists");
            rows.remove(field, key);
            if rows.0.is_empty() {
                self.repository_rows.remove_discard(repository);
            }
        }
    }
    fn branch_dependents(&self, branch: &str, writes: &mut Writes) {
        writes.row(Field::FileBranchAliases, branch);
        self.selected_files
            .each(branch, |key| writes.row(Field::FileSelections, key));
    }
    fn directory_branch_dependents(&self, branch: &str, writes: &mut Writes) {
        self.directory_trees
            .each(branch, |key| writes.row(Field::DirectoryTrees, key));
        self.selected_directories
            .each(branch, |key| writes.row(Field::DirectorySelections, key));
    }
    fn repository_dependents(&self, repository: &str, writes: &mut Writes) {
        if let Some(rows) = self.repository_rows.get(repository) {
            rows.dirty(writes);
            if let Some(branches) = rows.0.get(&Field::DirectoryBranches) {
                for branch in branches.keys() {
                    self.directory_branch_dependents(branch, writes);
                }
            }
        }
    }
}

impl ProjectionDelta {
    /// None denotes an actual All/foreign mutable escape, never a known ordinary write.
    pub(super) fn between(
        before: &State,
        after: &State,
        index: &DerivedOwners,
    ) -> Result<Option<Self>> {
        let mut histories = BTreeMap::new();
        Ok(
            DerivedOwners::transition(before, after, index, Some(&mut histories))?
                .map(|owners| Self { owners, histories }),
        )
    }
}

impl DerivedOwners {
    /// Membership updates are pure State derivations. Only a caller with a
    /// published history basis requests the fallible JSON projection as well.
    fn transition(
        before: &State,
        after: &State,
        index: &Self,
        mut histories: Option<&mut BTreeMap<String, HistoryEdits>>,
    ) -> Result<Option<Self>> {
        let Some(writes) = Writes::between(before, after) else {
            return Ok(None);
        };
        let mut affected = writes.clone();
        let mut owners = index.clone();
        owners.update_dependencies(before, after, &writes, &mut affected);
        owners.update_file_rows(before, after, index, &writes);
        owners.update_deletion_rows(before, after, &writes);
        if !writes.histories.is_empty() {
            // Unknown owners must remain visible to every history admission.
            // Only these indexed malformed carriers depend on the history set.
            for source in [index, &owners] {
                for id in source.unscoped_path_audit.keys() {
                    affected.row(Field::PathAudit, &id.to_string());
                }
            }
        }
        for key in &writes.histories {
            if before.histories.get(key) != after.histories.get(key)
                && let Some(histories) = histories.as_deref_mut()
            {
                let edits = histories.entry(key.clone()).or_default();
                edits.history = true;
                edits.repository = true;
            }
            let previous = before
                .histories
                .get(key)
                .map(|history| history.repository_id.as_str());
            let next = after
                .histories
                .get(key)
                .map(|history| history.repository_id.as_str());
            if previous != next {
                for repository in previous.into_iter().chain(next) {
                    index.repository_dependents(repository, &mut affected);
                    owners.repository_dependents(repository, &mut affected);
                }
            }
        }
        for key in &writes.repositories {
            if before.repositories.get(key) != after.repositories.get(key)
                && let Some(histories) = histories.as_deref_mut()
            {
                for source in [index, &owners] {
                    source.repositories.each(key, |history| {
                        histories.entry(history.to_owned()).or_default().repository = true;
                    });
                }
            }
        }
        for key in &writes.rows[&Field::Branches] {
            if before
                .branches
                .get(key)
                .map(|branch| &branch.history_tree_id)
                != after
                    .branches
                    .get(key)
                    .map(|branch| &branch.history_tree_id)
            {
                index.branch_dependents(key, &mut affected);
                owners.branch_dependents(key, &mut affected);
            }
        }
        for key in &writes.rows[&Field::DirectoryBranches] {
            if before
                .directory_branches
                .get(key)
                .map(|branch| &branch.repository_id)
                != after
                    .directory_branches
                    .get(key)
                    .map(|branch| &branch.repository_id)
            {
                index.directory_branch_dependents(key, &mut affected);
                owners.directory_branch_dependents(key, &mut affected);
            }
        }
        for field in [Field::FilePathNodes, Field::FileMetadataNodes] {
            for key in &writes.rows[&field] {
                affected.row(Field::FilePathNotes, key);
            }
        }
        for (field, keys) in affected.rows {
            for key in keys {
                let previous = field.owners(before, index, &key);
                let next = field.owners(after, &owners, &key);
                for history in previous.difference(&next) {
                    owners.member(history, field, &key, false);
                }
                for history in next.difference(&previous) {
                    owners.member(history, field, &key, true);
                }
                let Some(histories) = histories.as_deref_mut() else {
                    continue;
                };
                let equal = field.equal(before, after, &key);
                if equal && previous == next {
                    continue;
                }
                let value = if next.is_empty() {
                    None
                } else {
                    field.value(after, &key)?
                };
                for history in previous.union(&next) {
                    if equal && previous.contains(history) && next.contains(history) {
                        continue;
                    }
                    let replacement = if next.contains(history) {
                        value.clone()
                    } else {
                        None
                    };
                    histories
                        .entry(history.clone())
                        .or_default()
                        .maps
                        .entry(field)
                        .or_default()
                        .insert(key.clone(), replacement);
                }
            }
        }
        Ok(Some(owners))
    }
}

fn repository_key<'a>(state: &'a State, field: Field, key: &'a str) -> Option<&'a str> {
    match field {
        Field::DirectoryBranches => state
            .directory_branches
            .get(key)
            .map(|r| r.repository_id.as_str()),
        Field::PlacementRules => state
            .placement_rules
            .get(key)
            .map(|r| r.repository_id.as_str()),
        Field::ProtectionPolicies => state
            .protection_policies
            .get(key)
            .map(|r| r.repository_id.as_str()),
        Field::FileContexts => state
            .file_contexts
            .contains_key(key)
            .then(|| key.split_once('/').map(|(r, _)| r))
            .flatten(),
        _ => unreachable!("invariant: only direct repository rows enter this reverse index"),
    }
}

fn change_reference(index: &mut References, key: &str, previous: Option<&str>, next: Option<&str>) {
    if previous == next {
        return;
    }
    if let Some(previous) = previous {
        index.remove(previous, key);
    }
    if let Some(next) = next {
        index.add(next, key);
    }
}

fn commit_references(commit: Option<&storage_contract::Commit>) -> BTreeMap<(&str, &str), usize> {
    let mut references = BTreeMap::new();
    if let Some(commit) = commit {
        for entry in &commit.entries {
            if entry.kind == NamespaceEntryKind::Directory {
                *references
                    .entry((entry.entry_id.as_str(), commit.history_tree_id.as_str()))
                    .or_default() += 1;
            }
        }
    }
    references
}

impl DerivedOwners {
    fn update_dependencies(
        &mut self,
        before: &State,
        after: &State,
        writes: &Writes,
        affected: &mut Writes,
    ) {
        for key in &writes.histories {
            let previous = before.histories.get(key).map(|h| h.repository_id.as_str());
            let next = after.histories.get(key).map(|h| h.repository_id.as_str());
            change_reference(&mut self.repositories, key, previous, next);
            change_reference(&mut self.directories, key, previous, next);
            if previous != next {
                for repository in previous.into_iter().chain(next) {
                    affected.directory(repository);
                }
            }
        }
        for key in &writes.rows[&Field::Versions] {
            let previous = before
                .versions
                .get(key)
                .map(|v| (v.file_id.as_str(), v.history_tree_id.as_str()));
            let next = after
                .versions
                .get(key)
                .map(|v| (v.file_id.as_str(), v.history_tree_id.as_str()));
            if previous != next {
                if let Some((file, history)) = previous
                    && self.files.remove(file, history, key)
                {
                    affected.file(file);
                }
                if let Some((file, history)) = next
                    && self.files.add(file, history, key)
                {
                    affected.file(file);
                }
            }
            if before
                .versions
                .get(key)
                .map(|version| &version.history_tree_id)
                != after
                    .versions
                    .get(key)
                    .map(|version| &version.history_tree_id)
            {
                affected.version(key);
            }
        }
        for key in &writes.rows[&Field::Commits] {
            if before.commits.get(key) == after.commits.get(key) {
                continue;
            }
            let previous = commit_references(before.commits.get(key));
            let next = commit_references(after.commits.get(key));
            for (&(directory, history), &count) in &previous {
                let retained = next.get(&(directory, history)).copied().unwrap_or(0);
                for _ in retained..count {
                    if self.directories.remove(directory, history) {
                        affected.directory(directory);
                    }
                }
            }
            for (&(directory, history), &count) in &next {
                let retained = previous.get(&(directory, history)).copied().unwrap_or(0);
                for _ in retained..count {
                    if self.directories.add(directory, history) {
                        affected.directory(directory);
                    }
                }
            }
        }
        for key in &writes.rows[&Field::FileSelections] {
            change_reference(
                &mut self.selected_files,
                key,
                before.file_selections.get(key).map(String::as_str),
                after.file_selections.get(key).map(String::as_str),
            );
        }
        for key in &writes.rows[&Field::DirectorySelections] {
            change_reference(
                &mut self.selected_directories,
                key,
                before.directory_selections.get(key).map(String::as_str),
                after.directory_selections.get(key).map(String::as_str),
            );
        }
        for key in &writes.rows[&Field::DirectoryTrees] {
            change_reference(
                &mut self.directory_trees,
                key,
                before
                    .directory_trees
                    .get(key)
                    .map(|tree| tree.directory_branch_id.as_str()),
                after
                    .directory_trees
                    .get(key)
                    .map(|tree| tree.directory_branch_id.as_str()),
            );
        }
        for field in [
            Field::DirectoryBranches,
            Field::PlacementRules,
            Field::ProtectionPolicies,
            Field::FileContexts,
        ] {
            for key in &writes.rows[&field] {
                if repository_key(before, field, key) == repository_key(after, field, key) {
                    continue;
                }
                self.repository_row(before, field, key, false);
                self.repository_row(after, field, key, true);
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/state_projection/container_membership_tests.rs"]
mod container_membership_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/state_projection/move_receipts_tests.rs"]
mod move_receipts_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/state_projection/audit_tests.rs"]
mod audit_tests;
