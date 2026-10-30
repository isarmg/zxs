//! Identifier-only memberships shared by history and container projection.
use super::{DerivedOwners, Field, FileVersions, State, Writes};
use std::collections::BTreeSet;
use storage_contract::SharedRecords;

// Nonempty version buckets also supply the file/history owner relation;
// there is no parallel reference counter to reconcile.
impl FileVersions {
    pub(super) fn add(&mut self, file: &str, history: &str, version: &str) -> bool {
        if !self.0.contains_key(file) {
            self.0.insert(file.to_owned(), SharedRecords::new());
        }
        let histories = self.0.get_mut(file).expect("invariant: file bucket exists");
        let introduced = !histories.contains_key(history);
        if introduced {
            histories.insert(history.to_owned(), SharedRecords::new());
        }
        let versions = histories
            .get_mut(history)
            .expect("invariant: history bucket exists");
        assert!(
            versions.insert(version.to_owned(), ()).is_none(),
            "invariant: each version contributes one membership"
        );
        introduced
    }

    pub(super) fn remove(&mut self, file: &str, history: &str, version: &str) -> bool {
        let histories = self
            .0
            .get_mut(file)
            .expect("invariant: old version has its file bucket");
        let versions = histories
            .get_mut(history)
            .expect("invariant: old version has its history bucket");
        assert!(
            versions.remove_discard(version),
            "invariant: old version contributes its membership"
        );
        let removed = versions.is_empty();
        if removed {
            histories.remove_discard(history);
        }
        if histories.is_empty() {
            self.0.remove_discard(file);
        }
        removed
    }

    pub(super) fn histories(&self, file: &str) -> impl Iterator<Item = &String> {
        self.0.get(file).into_iter().flat_map(SharedRecords::keys)
    }

    pub(super) fn versions(&self, file: &str) -> impl Iterator<Item = &String> {
        self.0
            .get(file)
            .into_iter()
            .flat_map(SharedRecords::values)
            .flat_map(SharedRecords::keys)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FileRows {
    pub(crate) branches: SharedRecords<()>,
    pub(crate) pending_branches: SharedRecords<()>,
    pub(crate) path_nodes: SharedRecords<()>,
    pub(crate) metadata_nodes: SharedRecords<()>,
    pub(crate) merges: SharedRecords<()>,
    pub(crate) contexts: SharedRecords<()>,
    pub(crate) aliases: SharedRecords<()>,
}

#[derive(Clone, Copy)]
enum FileField {
    Branch,
    PendingBranch,
    PathNode,
    MetadataNode,
    Merge,
    Context,
    Alias,
}

impl FileRows {
    fn field(&mut self, field: FileField) -> &mut SharedRecords<()> {
        match field {
            FileField::Branch => &mut self.branches,
            FileField::PendingBranch => &mut self.pending_branches,
            FileField::PathNode => &mut self.path_nodes,
            FileField::MetadataNode => &mut self.metadata_nodes,
            FileField::Merge => &mut self.merges,
            FileField::Context => &mut self.contexts,
            FileField::Alias => &mut self.aliases,
        }
    }

    fn is_empty(&self) -> bool {
        let Self {
            branches,
            pending_branches,
            path_nodes,
            metadata_nodes,
            merges,
            contexts,
            aliases,
        } = self;
        branches.is_empty()
            && pending_branches.is_empty()
            && path_nodes.is_empty()
            && metadata_nodes.is_empty()
            && merges.is_empty()
            && contexts.is_empty()
            && aliases.is_empty()
    }
}

fn context_file<'a>(state: &'a State, key: &str) -> Option<&'a str> {
    let context = state.file_contexts.get(key)?;
    state.branches.get(&context.branch_id)?.file_id.as_deref()
}

fn alias_file<'a>(state: &'a State, key: &str) -> Option<&'a str> {
    state.file_branch_aliases.get(key)?;
    state.branches.get(key)?.file_id.as_deref()
}

impl DerivedOwners {
    /// Smallest version id across this file's nonempty history buckets.
    pub(crate) fn first_version(&self, file: &str) -> Option<&String> {
        self.files
            .0
            .get(file)?
            .values()
            .map(|versions| {
                versions
                    .keys()
                    .next()
                    .expect("invariant: file/history version buckets are nonempty")
            })
            .min()
    }

    pub(crate) fn version_membership(
        &self,
        file: &str,
    ) -> Option<&SharedRecords<SharedRecords<()>>> {
        self.files.0.get(file)
    }

    pub(crate) fn versions(&self, file: &str) -> impl Iterator<Item = &String> {
        self.files.versions(file)
    }

    pub(crate) fn file_histories(&self, file: &str) -> impl Iterator<Item = &String> {
        self.files.histories(file)
    }

    pub(crate) fn file_rows(&self, file: &str) -> Option<&FileRows> {
        self.file_rows.get(file)
    }

    fn file_member(&mut self, file: &str, field: FileField, key: &str, add: bool) {
        if add {
            if !self.file_rows.contains_key(file) {
                self.file_rows.insert(file.to_owned(), FileRows::default());
            }
            let rows = self
                .file_rows
                .get_mut(file)
                .expect("invariant: file row group exists");
            assert!(
                rows.field(field).insert(key.to_owned(), ()).is_none(),
                "invariant: row contributes once to its file"
            );
        } else {
            let rows = self
                .file_rows
                .get_mut(file)
                .expect("invariant: old row has its file group");
            assert!(
                rows.field(field).remove_discard(key),
                "invariant: old row contributes to its file"
            );
            if rows.is_empty() {
                self.file_rows.remove_discard(file);
            }
        }
    }

    fn move_file_member(
        &mut self,
        field: FileField,
        key: &str,
        previous: Option<&str>,
        next: Option<&str>,
    ) {
        if previous != next {
            if let Some(file) = previous {
                self.file_member(file, field, key, false);
            }
            if let Some(file) = next {
                self.file_member(file, field, key, true);
            }
        }
    }

    fn branch_context(&mut self, branch: &str, key: &str, add: bool) {
        if add {
            if !self.branch_contexts.contains_key(branch) {
                self.branch_contexts
                    .insert(branch.to_owned(), SharedRecords::new());
            }
            assert!(
                self.branch_contexts
                    .get_mut(branch)
                    .expect("invariant: context branch group exists")
                    .insert(key.to_owned(), ())
                    .is_none(),
                "invariant: context has one branch membership"
            );
        } else {
            let keys = self
                .branch_contexts
                .get_mut(branch)
                .expect("invariant: old context has its branch group");
            assert!(
                keys.remove_discard(key),
                "invariant: old context contributes to its branch"
            );
            if keys.is_empty() {
                self.branch_contexts.remove_discard(branch);
            }
        }
    }

    pub(super) fn build_file_rows(
        &mut self,
        state: &State,
        observe: &dyn Fn() -> crate::Result<()>,
    ) -> crate::Result<()> {
        for (key, branch) in state.branches.iter() {
            observe()?;
            if let Some(file) = &branch.file_id {
                self.file_member(file, FileField::Branch, key, true);
            }
            for file in branch.pending_file_versions.keys() {
                observe()?;
                self.file_member(file, FileField::PendingBranch, key, true);
            }
        }
        macro_rules! direct {
            ($field:ident, $kind:ident) => {
                for (key, row) in state.$field.iter() {
                    observe()?;
                    self.file_member(&row.file_id, FileField::$kind, key, true);
                }
            };
        }
        direct!(file_path_nodes, PathNode);
        direct!(file_metadata_nodes, MetadataNode);
        direct!(file_branch_merges, Merge);
        for (key, context) in state.file_contexts.iter() {
            observe()?;
            self.branch_context(&context.branch_id, key, true);
            if let Some(file) = context_file(state, key) {
                self.file_member(file, FileField::Context, key, true);
            }
        }
        for key in state.file_branch_aliases.keys() {
            observe()?;
            if let Some(file) = alias_file(state, key) {
                self.file_member(file, FileField::Alias, key, true);
            }
        }
        Ok(())
    }

    pub(super) fn update_file_rows(
        &mut self,
        before: &State,
        after: &State,
        old: &Self,
        writes: &Writes,
    ) {
        for key in &writes.rows[&Field::Branches] {
            let previous = before.branches.get(key);
            let next = after.branches.get(key);
            self.move_file_member(
                FileField::Branch,
                key,
                previous.and_then(|branch| branch.file_id.as_deref()),
                next.and_then(|branch| branch.file_id.as_deref()),
            );
            let previous_files = previous
                .into_iter()
                .flat_map(|branch| branch.pending_file_versions.keys())
                .collect::<BTreeSet<_>>();
            let next_files = next
                .into_iter()
                .flat_map(|branch| branch.pending_file_versions.keys())
                .collect::<BTreeSet<_>>();
            for file in previous_files.difference(&next_files) {
                self.file_member(file, FileField::PendingBranch, key, false);
            }
            for file in next_files.difference(&previous_files) {
                self.file_member(file, FileField::PendingBranch, key, true);
            }
        }
        macro_rules! direct {
            ($field:ident, $kind:ident, $membership:ident) => {
                for key in &writes.rows[&Field::$kind] {
                    self.move_file_member(
                        FileField::$membership,
                        key,
                        before.$field.get(key).map(|row| row.file_id.as_str()),
                        after.$field.get(key).map(|row| row.file_id.as_str()),
                    );
                }
            };
        }
        direct!(file_path_nodes, FilePathNodes, PathNode);
        direct!(file_metadata_nodes, FileMetadataNodes, MetadataNode);
        direct!(file_branch_merges, FileBranchMerges, Merge);
        let mut contexts = writes.rows[&Field::FileContexts].clone();
        for key in &writes.rows[&Field::FileContexts] {
            let previous = before
                .file_contexts
                .get(key)
                .map(|context| context.branch_id.as_str());
            let next = after
                .file_contexts
                .get(key)
                .map(|context| context.branch_id.as_str());
            if previous != next {
                if let Some(branch) = previous {
                    self.branch_context(branch, key, false);
                }
                if let Some(branch) = next {
                    self.branch_context(branch, key, true);
                }
            }
        }
        let mut aliases = writes.rows[&Field::FileBranchAliases].clone();
        for key in &writes.rows[&Field::Branches] {
            if before
                .branches
                .get(key)
                .and_then(|branch| branch.file_id.as_ref())
                == after
                    .branches
                    .get(key)
                    .and_then(|branch| branch.file_id.as_ref())
            {
                continue;
            }
            aliases.insert(key.clone());
            for index in [old, &*self] {
                if let Some(keys) = index.branch_contexts.get(key) {
                    contexts.extend(keys.keys().cloned());
                }
            }
        }
        for key in contexts {
            self.move_file_member(
                FileField::Context,
                &key,
                context_file(before, &key),
                context_file(after, &key),
            );
        }
        for key in aliases {
            self.move_file_member(
                FileField::Alias,
                &key,
                alias_file(before, &key),
                alias_file(after, &key),
            );
        }
    }
}
