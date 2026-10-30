//! Shared State-derived membership and exact projection closure over map writes.
//!
//! The owner index is derived only from State. Its publication lifetime is bound
//! by PublishedStateBase, never by SQLite, sequence equality or a graph hash.
use crate::{Result, State};
use json_codec::Value;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{ContentState, MapChanges, SharedRecords};
mod files;
pub(crate) use files::FileRows;

// One schema drives State coverage, row lookup, membership, and graph field names.
macro_rules! projection_maps {
    ($consumer:ident) => { $consumer! {
        branches => Branches, history, "branches";
        file_selections => FileSelections, selected_file, "file_selections";
        directory_branches => DirectoryBranches, repository, "directory_branches";
        directory_trees => DirectoryTrees, directory_tree, "directory_trees";
        file_write_origins => FileWriteOrigins, file, "file_write_origins";
        file_contexts => FileContexts, context, "file_contexts";
        file_path_nodes => FilePathNodes, history, "file_path_nodes";
        file_metadata_nodes => FileMetadataNodes, history, "file_metadata_nodes";
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
        pub(super) enum Field { $($variant,)* }
        impl Field {
            pub(super) const ALL: &'static [Self] = &[$(Self::$variant,)*];
            pub(super) fn name(self) -> &'static str {
                match self { $(Self::$variant => $name,)* }
            }
            fn keys(self, state: &State, mut visit: impl FnMut(&str) -> Result<()>) -> Result<()> {
                match self { $(Self::$variant => field_keys!($field, state, visit),)* }
                Ok(())
            }
            pub(super) fn value(self, state: &State, key: &str) -> Result<Option<Value>> {
                match self { $(Self::$variant => field_value!($field, state, key),)* }
            }
            fn equal(self, before: &State, after: &State, key: &str) -> bool {
                match self { $(Self::$variant => field_equal!($field, before, after, key),)* }
            }
            fn owners(self, state: &State, index: &DerivedOwners, key: &str) -> BTreeSet<String> {
                if !self.present(state, key) { return BTreeSet::new(); }
                match self { $(Self::$variant => owner_for!($owner, state, index, key, $field),)* }
            }
            fn present(self, state: &State, key: &str) -> bool {
                match self { $(Self::$variant => field_present!($field, state, key),)* }
            }
        }
        impl Writes {
            fn between(before: &State, after: &State) -> Option<Self> {
                let State {
                    sequence: _, business_generation: _, topology_generation: _,
                    control_auth: _, events: _, operations: _, plans: _,
                    histories: _, repositories: _, $($field: _,)*
                } = after;
                let mut writes = Self {
                    histories: exact_keys(after.histories.changes_since(&before.histories))?,
                    repositories: exact_keys(after.repositories.changes_since(&before.repositories))?,
                    ..Self::default()
                };
                $(writes.rows.insert(Field::$variant,
                    exact_keys(after.$field.changes_since(&before.$field))?);)*
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
    (content_states, $state:ident, $visit:ident) => {
        for key in $state.versions.keys() {
            $visit(key)?;
        }
    };
    ($field:ident, $state:ident, $visit:ident) => {
        for key in $state.$field.keys() {
            $visit(key)?;
        }
    };
}
macro_rules! field_value {
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
    (content_states, $before:ident, $after:ident, $key:ident) => {
        $before.versions.contains_key($key) == $after.versions.contains_key($key)
            && content_state($before, $key) == content_state($after, $key)
    };
    ($field:ident, $before:ident, $after:ident, $key:ident) => {
        $before.$field.get($key) == $after.$field.get($key)
    };
}
macro_rules! field_present {
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
fn exact_keys(changes: MapChanges) -> Option<BTreeSet<String>> {
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
    pub(super) fn keys(&self, field: Field) -> impl Iterator<Item = &String> {
        self.0.get(&field).into_iter().flat_map(SharedRecords::keys)
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

/// The earliest commit that published a version: content nodes take their
/// display sequence, path and message from it. Ties on sequence keep the
/// smallest commit id so the choice is a pure function of State.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FirstCommit {
    pub(crate) sequence: u64,
    pub(crate) commit_id: String,
}

impl FirstCommit {
    fn precedes(&self, other: &Self) -> bool {
        (self.sequence, &self.commit_id) < (other.sequence, &other.commit_id)
    }
}

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
    /// version id -> first publishing commit, derived from `State::commits`.
    first_commits: SharedRecords<FirstCommit>,
    /// Namespace object -> earliest visiting commit. Shared objects can be
    /// skipped before I/O on append; removal/rewrite rebuilds both indexes.
    namespace_first_commits: SharedRecords<FirstCommit>,
}

#[derive(Clone, Default)]
struct Writes {
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
}

#[derive(Clone, Copy)]
pub(crate) struct StateProjection<'a> {
    state: &'a State,
    owners: &'a DerivedOwners,
}

impl<'a> StateProjection<'a> {
    pub(crate) fn state(self) -> &'a State {
        self.state
    }
    pub(crate) fn owners(self) -> &'a DerivedOwners {
        self.owners
    }
}

impl<'a> ProjectionTransition<'a> {
    pub(crate) fn new(
        before: &'a State,
        after: &'a State,
        baseline: Option<&'a DerivedOwners>,
        source: &dyn crate::namespace::NamespaceSource,
    ) -> Result<Self> {
        let previous = match baseline {
            Some(index) => std::borrow::Cow::Borrowed(index),
            None => std::borrow::Cow::Owned(DerivedOwners::from_state(before, source)?),
        };
        let delta = match baseline {
            Some(index) => ProjectionDelta::between(before, after, index, source)?,
            None => None,
        };
        let (next, history_edits) = match delta {
            Some(delta) => (delta.owners, Some(delta.histories)),
            None => (DerivedOwners::from_state(after, source)?, None),
        };
        Ok(Self {
            before,
            after,
            previous,
            next,
            history_edits,
        })
    }

    pub(crate) fn previous(&self) -> StateProjection<'_> {
        StateProjection {
            state: self.before,
            owners: &self.previous,
        }
    }

    pub(crate) fn next(&self) -> StateProjection<'_> {
        StateProjection {
            state: self.after,
            owners: &self.next,
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
    pub(crate) fn from_state(
        state: &State,
        source: &dyn crate::namespace::NamespaceSource,
    ) -> crate::Result<Self> {
        Self::from_state_observed(state, source, &|| Ok(()))
    }

    /// Build a complete immutable derivation before its owner publishes it.
    /// Observation errors discard this local index, never a cached partial value.
    pub(crate) fn from_state_observed(
        state: &State,
        source: &dyn crate::namespace::NamespaceSource,
        observe: &dyn Fn() -> Result<()>,
    ) -> crate::Result<Self> {
        observe()?;
        let mut index = Self::default();
        for history in state.histories.values() {
            observe()?;
            index
                .repositories
                .add(&history.repository_id, &history.history_tree_id);
            index
                .directories
                .add(&history.repository_id, &history.history_tree_id);
        }
        for (key, version) in state.versions.iter() {
            observe()?;
            index
                .files
                .add(&version.file_id, &version.history_tree_id, key);
        }
        // Stream every commit's directory references through the paged
        // namespace; object reads share the bounded cache, and the full
        // enumeration stays confined to this rebuild-from-scratch entry.
        for commit in state.commits.values() {
            observe()?;
            crate::namespace::directory_reference_deltas(
                None,
                Some((&commit.namespace, &commit.history_tree_id)),
                source,
                &mut |entry_id, history, _| {
                    observe()?;
                    index.directories.add(entry_id, history);
                    Ok(())
                },
            )?;
            index.commit_first_versions(commit, source)?;
        }
        for (key, branch) in state.file_selections.iter() {
            observe()?;
            index.selected_files.add(branch, key);
        }
        for (key, branch) in state.directory_selections.iter() {
            observe()?;
            index.selected_directories.add(branch, key);
        }
        for (key, tree) in state.directory_trees.iter() {
            observe()?;
            index.directory_trees.add(&tree.directory_branch_id, key);
        }
        for field in [
            Field::DirectoryBranches,
            Field::PlacementRules,
            Field::ProtectionPolicies,
            Field::FileContexts,
        ] {
            field.keys(state, |key| {
                observe()?;
                index.repository_row(state, field, key, true);
                Ok(())
            })?;
        }
        for &field in Field::ALL {
            field.keys(state, |key| {
                observe()?;
                for history in field.owners(state, &index, key) {
                    observe()?;
                    index.member(&history, field, key, true);
                }
                Ok(())
            })?;
        }
        index.build_file_rows(state, observe)?;
        observe()?;
        Ok(index)
    }
    pub(super) fn rows(&self, history: &str) -> Option<&Rows> {
        self.history_rows.get(history)
    }
    /// The earliest commit publishing this version, if any commit references it.
    pub(crate) fn first_commit(&self, version: &str) -> Option<&FirstCommit> {
        self.first_commits.get(version)
    }
    /// Every version inside an already-seen page/index has an equal or
    /// earlier first publisher. Shared references prune before object reads;
    /// request-local work never allocates a directory/history-sized frontier.
    fn commit_first_versions(
        &mut self,
        commit: &storage_contract::Commit,
        source: &dyn crate::namespace::NamespaceSource,
    ) -> Result<()> {
        let candidate = FirstCommit {
            sequence: commit.sequence,
            commit_id: commit.commit_id.clone(),
        };
        let versions = &mut self.first_commits;
        let namespaces = &mut self.namespace_first_commits;
        crate::namespace::visit_file_versions(
            &commit.namespace,
            source,
            &mut |reference| {
                let key = format!("{}:{}", reference.kind.as_str(), reference.hash);
                if namespaces
                    .get(&key)
                    .is_some_and(|old| !candidate.precedes(old))
                {
                    return false;
                }
                namespaces.insert(key, candidate.clone());
                true
            },
            &mut |version| {
                if versions
                    .get(version)
                    .is_none_or(|old| candidate.precedes(old))
                {
                    versions.insert(version.to_owned(), candidate.clone());
                }
            },
        )
    }

    /// Immutable commits only append in ordinary publication. Removal or
    /// rewrite is bulk maintenance and invalidates namespace first witnesses;
    /// rebuild both indexes together, never retain a stale pruning witness.
    fn rebuild_first_versions(
        &mut self,
        state: &State,
        source: &dyn crate::namespace::NamespaceSource,
    ) -> Result<()> {
        self.first_commits = SharedRecords::new();
        self.namespace_first_commits = SharedRecords::new();
        for commit in state.commits.values() {
            self.commit_first_versions(commit, source)?;
        }
        Ok(())
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
        source: &dyn crate::namespace::NamespaceSource,
    ) -> Result<Option<Self>> {
        let Some(writes) = Writes::between(before, after) else {
            return Ok(None);
        };
        let mut affected = writes.clone();
        let mut owners = index.clone();
        owners.update_dependencies(before, after, &writes, &mut affected, source)?;
        owners.update_file_rows(before, after, index, &writes);
        let mut histories = BTreeMap::<String, HistoryEdits>::new();
        for key in &writes.histories {
            if before.histories.get(key) != after.histories.get(key) {
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
            if before.repositories.get(key) != after.repositories.get(key) {
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
        Ok(Some(Self { owners, histories }))
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

impl DerivedOwners {
    fn update_dependencies(
        &mut self,
        before: &State,
        after: &State,
        writes: &Writes,
        affected: &mut Writes,
        source: &dyn crate::namespace::NamespaceSource,
    ) -> crate::Result<()> {
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
        let rebuild_first = writes.rows[&Field::Commits].iter().any(|key| {
            before.commits.contains_key(key) && before.commits.get(key) != after.commits.get(key)
        });
        for key in &writes.rows[&Field::Commits] {
            if before.commits.get(key) == after.commits.get(key) {
                continue;
            }
            if !rebuild_first && let Some(next) = after.commits.get(key) {
                self.commit_first_versions(next, source)?;
            }
            // Net reference deltas from the reference-pruned namespace diff:
            // shared subtrees contribute identically on both sides and are
            // never read, so an edit costs its changed paths only.
            let mut deltas = BTreeMap::<(String, String), i64>::new();
            crate::namespace::directory_reference_deltas(
                before
                    .commits
                    .get(key)
                    .map(|commit| (&commit.namespace, commit.history_tree_id.as_str())),
                after
                    .commits
                    .get(key)
                    .map(|commit| (&commit.namespace, commit.history_tree_id.as_str())),
                source,
                &mut |entry_id, history, delta| {
                    *deltas
                        .entry((entry_id.to_owned(), history.to_owned()))
                        .or_default() += delta;
                    Ok(())
                },
            )?;
            for ((directory, history), net) in deltas {
                for _ in 0..net.unsigned_abs() {
                    let changed = if net < 0 {
                        self.directories.remove(&directory, &history)
                    } else {
                        self.directories.add(&directory, &history)
                    };
                    if changed {
                        affected.directory(&directory);
                    }
                }
            }
        }
        if rebuild_first {
            self.rebuild_first_versions(after, source)?;
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
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/state_projection/container_membership_tests.rs"]
mod container_membership_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/state_projection/first_commit_tests.rs"]
mod first_commit_tests;
