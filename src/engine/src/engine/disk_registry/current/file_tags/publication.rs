//! Exact current metadata publication, without reading immutable raw namespaces.
use super::*;

impl Planner<'_> {
    pub(super) fn metadata_children(&mut self) -> Result<()> {
        for index in 0..self.changes.len() {
            if !self.changes[index].changed {
                continue;
            }
            let id = self.ids.next("fmeta")?;
            if self.previous.file_metadata_nodes.contains_key(&id) {
                return Err(invalid());
            }
            self.changes[index].node = id.clone();
            let change = &self.changes[index];
            let branch = &self.previous.branches[&change.branch];
            self.plan.file_metadata_nodes.insert(
                id.clone(),
                Some(FileMetadataNode {
                    node_id: id,
                    history_tree_id: branch.history_tree_id.clone(),
                    file_id: self.input.file_id.clone(),
                    version_id: change.version.clone(),
                    parent_node_ids: vec![change.source.clone()],
                    branch_id: change.branch.clone(),
                    kind: FileMetadataKind::Tags,
                    tags: change.tags.clone(),
                    operation_id: self.operation,
                    author: self.principal.into(),
                    sequence: self.sequence,
                    created_at: self.created_at,
                }),
            );
            if change.advance {
                let mut branch = branch.clone();
                let entries = self.changes[index].entries.take().expect(
                    "invariant: changed current tag target has an admitted source projection",
                );
                let commit = self.commit(
                    &branch.history_tree_id,
                    &branch.head_commit_id,
                    entries,
                    branch.pending_file_versions.clone(),
                    Some(self.changes[index].node.clone()),
                )?;
                branch.head_commit_id = commit;
                branch.file_node_id = Some(self.changes[index].node.clone());
                branch.generation = increment(branch.generation)?;
                self.plan
                    .branches
                    .insert(branch.branch_id.clone(), Some(branch));
            }
        }
        Ok(())
    }

    fn commit(
        &mut self,
        history: &str,
        parent: &str,
        entries: Vec<Entry>,
        pending: BTreeMap<String, String>,
        provenance: Option<String>,
    ) -> Result<String> {
        let id = self.ids.next("commit")?;
        if self.previous.commits.contains_key(&id) {
            return Err(invalid());
        }
        let tree = self.ids.next("tree")?;
        let kind = if provenance.is_some() {
            "file_metadata"
        } else {
            "file_node_publication"
        };
        self.plan.commits.insert(
            id.clone(),
            Some(Commit {
                namespace_deletions: None,
                commit_id: id.clone(),
                history_tree_id: history.into(),
                kind: kind.into(),
                parent_commit_ids: vec![parent.into()],
                tree_id: tree,
                sequence: self.sequence,
                operation_id: self.operation,
                author: self.principal.into(),
                message: "Publish current file tags".into(),
                entries,
                pending_file_versions: pending,
                provenance,
            }),
        );
        Ok(id)
    }

    fn matching(&self, entry: &Entry, context: &FileContext) -> Option<usize> {
        (entry.kind == NamespaceEntryKind::File && entry.file_id == self.input.file_id)
            .then(|| {
                self.changes.iter().position(|change| {
                    change.changed
                        && change.branch == context.branch_id
                        && change.source == context.node_id.as_deref().unwrap_or(&entry.version_id)
                })
            })
            .flatten()
    }

    fn tag_context(
        &self,
        repository: &Repository,
        entry: &Entry,
        context: &mut FileContext,
        index: usize,
    ) -> Result<()> {
        let owner = self
            .view
            .writable_scope(self.previous, &repository.main_branch_id)?;
        if !at_key(&owner.entries, entry.lookup_key())
            .is_some_and(|owner| same_occurrence(owner, entry))
            || entry.version_id != self.changes[index].version
        {
            return Err(missing());
        }
        super::super::directory_branches::validate_context(
            self.previous,
            &repository.repository_id,
            entry,
            context,
        )?;
        context.node_id = Some(self.changes[index].node.clone());
        context.selection_generation = increment(context.selection_generation)?;
        Ok(())
    }

    pub(super) fn publish_main(&mut self, repository: &Repository) -> Result<()> {
        let branch = &self.previous.branches[&repository.main_branch_id];
        let entries = self
            .view
            .branch_entries(&branch.branch_id, &branch.head_commit_id)?;
        let mut changed = false;
        for entry in entries.iter().filter(|entry| {
            entry.kind == NamespaceEntryKind::File && entry.file_id == self.input.file_id
        }) {
            let key = crate::business::context_key_from_key(
                &repository.repository_id,
                entry.lookup_key(),
            );
            let Some(context) = self.previous.file_contexts.get(&key) else {
                return Err(stale());
            };
            if let Some(index) = self.matching(entry, context) {
                let mut context = context.clone();
                self.tag_context(repository, entry, &mut context, index)?;
                self.plan.file_contexts.insert(key, Some(context));
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        let live: BTreeSet<_> = entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .map(|entry| entry.file_id.as_str())
            .collect();
        let mut pending = branch.pending_file_versions.clone();
        pending.retain(|file, _| live.contains(file.as_str()));
        let mut branch = branch.clone();
        let commit_pending = self.pending(entries, |entry| {
            self.previous
                .file_contexts
                .get(&crate::business::context_key_from_key(
                    &repository.repository_id,
                    entry.lookup_key(),
                ))
        })?;
        branch.head_commit_id = self.commit(
            &repository.history_tree_id,
            &branch.head_commit_id,
            entries.to_vec(),
            commit_pending,
            None,
        )?;
        branch.pending_file_versions = pending;
        branch.generation = increment(branch.generation)?;
        self.plan
            .branches
            .insert(branch.branch_id.clone(), Some(branch));
        for path in self.view.removed_paths(&repository.main_branch_id) {
            let key = crate::business::context_key_from_key(&repository.repository_id, path);
            if self.previous.file_contexts.contains_key(&key) {
                self.plan.file_contexts.insert(key, None);
            }
        }
        Ok(())
    }

    fn pending<'a>(
        &self,
        entries: &'a [Entry],
        context: impl Fn(&'a Entry) -> Option<&'a FileContext>,
    ) -> Result<BTreeMap<String, String>> {
        let mut pending = BTreeMap::new();
        for entry in entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            let context = context(entry).ok_or_else(stale)?;
            let branch = self
                .previous
                .branches
                .get(&context.branch_id)
                .ok_or_else(stale)?;
            if context.inherited_from.is_some()
                || branch.pending_file_versions.contains_key(&entry.file_id)
            {
                pending.insert(entry.file_id.clone(), entry.version_id.clone());
            }
        }
        Ok(pending)
    }

    pub(super) fn publish_directories(&mut self, repository: &Repository) -> Result<()> {
        // A source is keyed by its exact projected immutable contents, never by
        // a mutable named branch alone. Named heads and selected older trees can
        // legitimately represent different bodies or inheritance contexts.
        let mut cache = BTreeMap::new();
        self.publish_named_directories(repository, &mut cache)?;
        self.publish_selected_directories(repository, &mut cache)
    }

    fn matches_tree(&self, tree: &DirectoryTree) -> bool {
        tree.entries.iter().any(|entry| {
            tree.file_contexts
                .get(entry.lookup_key())
                .is_some_and(|context| self.matching(entry, context).is_some())
        })
    }

    fn publish_named_directories(
        &mut self,
        repository: &Repository,
        cache: &mut BTreeMap<String, (String, BTreeSet<usize>)>,
    ) -> Result<()> {
        for source in self
            .previous
            .directory_branches
            .values()
            .filter(|branch| branch.repository_id == repository.repository_id)
        {
            if !self
                .view
                .directory_entries(&source.directory_branch_id, &source.head_tree_id)?
                .iter()
                .any(|entry| {
                    entry.kind == NamespaceEntryKind::File && entry.file_id == self.input.file_id
                })
            {
                continue;
            }
            let projection = self
                .view
                .directory_projection(self.previous, &source.directory_branch_id)?;
            let mut nodes = projection
                .file_tag_trees()
                .map(|(directory, tree)| (directory.to_owned(), tree))
                .collect::<Vec<_>>();
            if !nodes.iter().any(|(_, tree)| self.matches_tree(tree)) {
                continue;
            }
            self.view
                .require_file_merge_directory(self.previous, projection.branch())?;
            // Every descendant comes from this root's authenticated flattened
            // graph, including any frozen child composition older than its HEAD.
            nodes.sort_by_key(|(_, tree)| tree.directory_refs.len());
            let mut rewritten = BTreeMap::new();
            for (directory, tree) in nodes {
                let result = self.rewrite_directory(repository, tree, &rewritten, cache)?;
                rewritten.insert(directory, result);
            }
            let (tree_id, matched) = &rewritten[&source.directory_id];
            if tree_id == &source.head_tree_id {
                continue;
            }
            let tree = self.plan.directory_trees[tree_id]
                .as_ref()
                .expect("invariant: rewritten directory tree is newly planned");
            let mut branch = source.clone();
            branch.head_tree_id = tree_id.clone();
            branch.commit_id = tree.commit_id.clone();
            branch.path = tree.path.clone();
            branch.entries = tree.entries.clone();
            branch.file_contexts = tree.file_contexts.clone();
            branch.file_branches = tree.file_branches.clone();
            branch.inherited_files = tree.inherited_files.clone();
            branch.directory_refs = tree.directory_refs.clone();
            branch.materialized = true;
            branch.sequence = self.sequence;
            for index in matched {
                self.changes[*index]
                    .directories
                    .insert(source.directory_branch_id.clone());
            }
            self.plan
                .directory_branches
                .insert(branch.directory_branch_id.clone(), Some(branch));
        }
        Ok(())
    }

    fn publish_selected_directories(
        &mut self,
        repository: &Repository,
        cache: &mut BTreeMap<String, (String, BTreeSet<usize>)>,
    ) -> Result<()> {
        let main = &self.previous.branches[&repository.main_branch_id];
        let owner = self
            .view
            .branch_entries(&main.branch_id, &main.head_commit_id)?;
        // Historical-only tags do not acquire a writable owner or make an
        // unrelated directory selection a new admission requirement.
        if !owner.iter().any(|entry| {
            self.previous
                .file_contexts
                .get(&crate::business::context_key_from_key(
                    &repository.repository_id,
                    entry.lookup_key(),
                ))
                .is_some_and(|context| self.matching(entry, context).is_some())
        }) {
            return Ok(());
        }
        let current_directories = owner
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
            .map(|entry| entry.entry_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut sources = Vec::new();
        for (directory, branch) in &self.previous.directory_selections {
            if self.previous.directory_branches[branch].repository_id != repository.repository_id {
                continue;
            }
            // Removed identities retain history, but no longer own a current scope.
            // The repository root is implicit and has no entry in its namespace.
            if directory != &repository.repository_id
                && !current_directories.contains(directory.as_str())
            {
                continue;
            }
            let scope = self.view.batch_scope(self.previous, branch)?;
            let branch = scope.branch();
            let original = self
                .previous
                .directory_trees
                .get(scope.selected_tree_id())
                .ok_or_else(stale)?;
            let tree = DirectoryTree {
                tree_id: original.tree_id.clone(),
                directory_branch_id: branch.directory_branch_id.clone(),
                parent_tree_id: original.parent_tree_id.clone(),
                commit_id: branch.commit_id.clone(),
                path: branch.path.clone(),
                entries: branch.entries.clone(),
                file_contexts: branch.file_contexts.clone(),
                file_branches: branch.file_branches.clone(),
                inherited_files: branch.inherited_files.clone(),
                directory_refs: branch.directory_refs.clone(),
                sequence: original.sequence,
            };
            sources.push((directory.clone(), tree));
        }
        sources.sort_by_key(|(_, tree)| tree.directory_refs.len());
        let mut affected = sources
            .iter()
            .filter(|(_, tree)| self.matches_tree(tree))
            .map(|(directory, _)| directory.clone())
            .collect::<BTreeSet<_>>();
        for (directory, tree) in sources.iter().rev() {
            if affected.contains(directory)
                || tree
                    .directory_refs
                    .keys()
                    .any(|child| affected.contains(child))
            {
                affected.insert(directory.clone());
                affected.extend(tree.directory_refs.keys().cloned());
            }
        }
        let mut rewritten = BTreeMap::new();
        for (directory, tree) in sources {
            if !affected.contains(&directory) {
                continue;
            }
            let original = tree.tree_id.clone();
            let result = self.rewrite_directory(repository, tree, &rewritten, cache)?;
            if result.0 != original {
                self.plan
                    .directory_tree_selections
                    .insert(directory.clone(), Some(result.0.clone()));
            }
            rewritten.insert(directory, result);
        }
        Ok(())
    }

    fn rewrite_directory(
        &mut self,
        repository: &Repository,
        mut tree: DirectoryTree,
        children: &BTreeMap<String, (String, BTreeSet<usize>)>,
        cache: &mut BTreeMap<String, (String, BTreeSet<usize>)>,
    ) -> Result<(String, BTreeSet<usize>)> {
        let mut matched = BTreeSet::new();
        for (directory, reference) in &mut tree.directory_refs {
            let (id, impacts) = children.get(directory).ok_or_else(stale)?;
            reference.tree_id = id.clone();
            matched.extend(impacts.iter().copied());
        }
        let key = crate::object_hash("current-file-tags-directory-source", &tree)?;
        if let Some(done) = cache.get(&key) {
            return Ok(done.clone());
        }
        for entry in &tree.entries {
            if let Some(context) = tree.file_contexts.get_mut(entry.lookup_key())
                && let Some(index) = self.matching(entry, context)
            {
                self.tag_context(repository, entry, context, index)?;
                matched.insert(index);
            }
        }
        let original = self
            .previous
            .directory_trees
            .get(&tree.tree_id)
            .ok_or_else(stale)?;
        let id = if tree == *original {
            tree.tree_id
        } else {
            self.freeze_directory(repository, tree)?
        };
        let result = (id, matched);
        cache.insert(key, result.clone());
        Ok(result)
    }

    fn freeze_directory(
        &mut self,
        repository: &Repository,
        mut tree: DirectoryTree,
    ) -> Result<String> {
        let owner = self
            .view
            .writable_scope(self.previous, &repository.main_branch_id)?;
        let mut full = tree.entries.clone();
        let key = name_policy::path_key(&tree.path);
        full.extend(
            owner
                .entries
                .iter()
                .filter(|entry| {
                    entry.kind == NamespaceEntryKind::Directory
                        && (entry.lookup_key() == key
                            || key
                                .strip_prefix(entry.lookup_key())
                                .is_some_and(|suffix| suffix.starts_with('/')))
                })
                .cloned(),
        );
        crate::business::validate_entries(&mut full, self.maximum)?;
        let pending = self.pending(&tree.entries, |entry| {
            tree.file_contexts.get(entry.lookup_key())
        })?;
        let commit = self.commit(
            &repository.history_tree_id,
            &tree.commit_id,
            full,
            pending,
            None,
        )?;
        let id = self.ids.next("dtree")?;
        if self.previous.directory_trees.contains_key(&id) {
            return Err(invalid());
        }
        tree.parent_tree_id = Some(tree.tree_id);
        tree.tree_id = id.clone();
        tree.commit_id = commit;
        tree.sequence = self.sequence;
        self.plan.directory_trees.insert(id.clone(), Some(tree));
        Ok(id)
    }
}
