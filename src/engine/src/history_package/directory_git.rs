//! Logical closure for explicit directory repositories in a retained history.
//! Working trees may precede initialization. Only user commit markers can be HEADs.
use super::*;
use storage_contract::directory_git::{DirectoryGitBranch, DirectoryGitRepository};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::HistoryPackageInvalid, message)
}

impl HistoryManifest {
    pub(super) fn validate_commit_forest(&self) -> Result<()> {
        for commit in self
            .commits
            .values()
            .filter(|commit| commit.kind == "directory_commit")
        {
            let tree = commit
                .provenance
                .as_ref()
                .and_then(|id| self.directory_trees.get(id))
                .ok_or_else(|| invalid("directory commit lacks its exact tree marker"))?;
            if tree.commit_id != commit.commit_id || !self.git_commit_marker(tree) {
                return Err(invalid("directory commit and its tree marker disagree"));
            }
        }
        let mut children = BTreeMap::<&str, Vec<&str>>::new();
        let mut pending = Vec::new();
        for (id, commit) in self.commits.iter() {
            if commit.parent_commit_ids.is_empty() {
                if id != &self.history.root_commit_id && commit.kind != "directory_commit" {
                    return Err(invalid("history has an unproven independent commit root"));
                }
                pending.push(id.as_str());
            }
            for parent in &commit.parent_commit_ids {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut reached = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !reached.insert(id) {
                continue;
            }
            if let Some(next) = children.get(id) {
                pending.extend(next.iter().copied());
            }
        }
        if reached.len() != self.commits.len() {
            return Err(invalid("history contains a cycle or detached commits"));
        }
        Ok(())
    }

    pub(super) fn validate_directory_git(&self) -> Result<()> {
        let paths = self.directory_git_paths();
        let namespace = self
            .branches
            .get(&self.repository.main_branch_id)
            .and_then(|branch| self.commits.get(&branch.head_commit_id))
            .ok_or_else(|| invalid("current namespace branch or commit is missing"))?;
        let live = namespace
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
            .map(|entry| entry.entry_id.as_str())
            .chain(std::iter::once(self.repository.repository_id.as_str()))
            .collect::<BTreeSet<_>>();
        for (directory, git) in &self.directory_git_repositories {
            if directory != &self.repository.repository_id {
                validate_id(directory, "entry")?;
            }
            if git.directory_id != *directory
                || git.repository_id != self.repository.repository_id
                || git.initialized_sequence == 0
                || git.branches.is_empty()
                || !git.branches.contains_key(&git.main_branch_id)
                || !git.branches.contains_key(&git.current_branch_id)
                || (directory != &self.repository.repository_id
                    && !paths.contains_key(directory.as_str()))
            {
                return Err(invalid(
                    "directory repository identity or initialization differs",
                ));
            }
            self.validate_directory_deletions(git, &namespace.entries)?;
            // Saves compose through directory_selections, while Git status and
            // commit use current_branch_id. Live repositories must agree on the
            // owner; retained deleted directories need no live selection.
            if live.contains(directory.as_str())
                && self.directory_selections.get(directory) != Some(&git.current_branch_id)
            {
                return Err(invalid(
                    "live directory Git branch differs from its working selection",
                ));
            }
            for (branch, index) in &git.branches {
                let composition = self
                    .directory_branches
                    .get(branch)
                    .ok_or_else(|| invalid("directory repository branch is missing"))?;
                if composition.directory_id != *directory
                    || composition.repository_id != git.repository_id
                    || index.index_generation == 0
                {
                    return Err(invalid(
                        "directory repository branch or index escapes its scope",
                    ));
                }
                self.git_scoped_tree(directory, branch, &index.index_tree_id)?;
                if let Some(head) = &index.head_tree_id {
                    self.git_committed_tree(directory, branch, head)?;
                }
                self.validate_git_index(git, index, &paths)?;
            }
        }
        for tree in self
            .directory_trees
            .values()
            .filter(|tree| self.git_commit_marker(tree))
        {
            self.validate_git_commit(tree)?;
        }
        self.validate_git_tree_cycles()
    }

    fn validate_directory_deletions(
        &self,
        git: &DirectoryGitRepository,
        current: &[Entry],
    ) -> Result<()> {
        let within = |path: &str, root: &str| {
            path == root
                || path
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        for (id, node) in &git.deletions {
            crate::file_deletion::directory::admit_node_shape(node)?;
            if id != &node.directory_id
                || node.repository_id != git.repository_id
                || node.history_tree_id != self.history.history_tree_id
                || node.owner_directory_id != git.directory_id
                || node.directory_id == git.directory_id
                || node.sequence <= git.initialized_sequence
                || !git.branches.contains_key(&node.directory_branch_id)
            {
                return Err(invalid(
                    "directory deletion owner or native identity differs",
                ));
            }
            let original = self
                .commits
                .get(&node.namespace_head)
                .ok_or_else(|| invalid("directory deletion has no retained namespace"))?;
            let source = self.git_scoped_tree(
                &git.directory_id,
                &node.directory_branch_id,
                &node.source_tree_id,
            )?;
            let root = original
                .entries
                .iter()
                .find(|entry| {
                    entry.entry_id == node.directory_id
                        && entry.path == node.path
                        && entry.kind == NamespaceEntryKind::Directory
                })
                .ok_or_else(|| {
                    invalid("deleted directory is absent from its original namespace")
                })?;
            if original.sequence >= node.sequence || source.sequence >= node.sequence {
                return Err(invalid(
                    "directory deletion source is not an earlier retained tree",
                ));
            }
            let before = original
                .entries
                .iter()
                .filter(|entry| within(entry.lookup_key(), root.lookup_key()))
                .collect::<Vec<_>>();
            let source_rows=crate::file_deletion::directory::retained_entries(node,original,source)?;
            if before!=source_rows.iter().collect::<Vec<_>>() {
                return Err(invalid("deleted directory retained contents changed"));
            }
            if let Some(mounted)=crate::file_deletion::directory::mounted_entries(node,original,source,current)? {
                let root=mounted.first().ok_or_else(||invalid("deleted directory retained root is missing"))?;
                let after=current.iter().filter(|entry|within(entry.lookup_key(),root.lookup_key())).collect::<Vec<_>>();
                if after!=mounted.iter().collect::<Vec<_>>() {
                    return Err(invalid("deleted directory current relative contents or owner changed"));
                }
            }
            let mut files = BTreeSet::new();
            for entry in before {
                if entry.kind == NamespaceEntryKind::Directory {
                    if self
                        .directory_git_repositories
                        .contains_key(&entry.entry_id)
                    {
                        return Err(invalid(
                            "directory deletion crosses an initialized child repository",
                        ));
                    }
                } else {
                    files.insert(entry.file_id.as_str());
                }
            }
            if files.into_iter().ne(node.files.keys().map(String::as_str)) {
                return Err(invalid("directory deletion member set differs"));
            }
            for (file, member) in &node.files {
                let deletion = self
                    .file_deletions
                    .get(file)
                    .ok_or_else(|| invalid("directory deletion member lacks its current file D"))?;
                if deletion.node_id != member.deletion_node_id
                    || (member.created_by_directory
                        && (deletion.operation_id != node.operation_id
                            || deletion.sequence != node.sequence
                            || deletion.author != node.author))
                    || (!member.created_by_directory && (deletion.sequence > node.sequence
                        || (deletion.sequence==node.sequence && deletion.operation_id!=node.operation_id)))
                {
                    return Err(invalid(
                        "directory deletion member's original lifecycle differs",
                    ));
                }
            }
        }
        Ok(())
    }

    fn directory_git_paths(&self) -> BTreeMap<&str, BTreeSet<&str>> {
        let mut paths = BTreeMap::<&str, BTreeSet<&str>>::new();
        for entry in self.commits.values().flat_map(|commit| &commit.entries) {
            if entry.kind == NamespaceEntryKind::Directory {
                paths
                    .entry(&entry.entry_id)
                    .or_default()
                    .insert(entry.lookup_key());
            }
        }
        paths
    }

    fn git_branch_inherits(&self, branch: &str, ancestor: &str) -> bool {
        // The enclosing manifest validator already admitted the complete,
        // acyclic composition-parent graph before checking these new edges.
        let mut cursor = Some(branch);
        while let Some(id) = cursor {
            if id == ancestor {
                return true;
            }
            cursor = self
                .directory_branches
                .get(id)
                .and_then(|branch| branch.parent_branch_id.as_deref());
        }
        false
    }

    fn git_scoped_tree(&self, directory: &str, branch: &str, tree: &str) -> Result<&DirectoryTree> {
        let tree = self
            .directory_trees
            .get(tree)
            .ok_or_else(|| invalid("directory Git tree is missing"))?;
        let owner = self
            .directory_branches
            .get(&tree.directory_branch_id)
            .ok_or_else(|| invalid("directory Git tree branch is missing"))?;
        if owner.directory_id != directory
            || owner.repository_id != self.repository.repository_id
            || !self.git_branch_inherits(branch, &tree.directory_branch_id)
        {
            return Err(invalid(
                "directory Git tree escapes its directory or branch ancestry",
            ));
        }
        Ok(tree)
    }

    fn git_commit_marker(&self, tree: &DirectoryTree) -> bool {
        self.commits.get(&tree.commit_id).is_some_and(|commit| {
            commit.kind == "directory_commit"
                && commit.provenance.as_deref() == Some(tree.tree_id.as_str())
        })
    }

    fn git_committed_tree(
        &self,
        directory: &str,
        branch: &str,
        tree: &str,
    ) -> Result<&DirectoryTree> {
        let tree = self.git_scoped_tree(directory, branch, tree)?;
        if !self.git_commit_marker(tree) {
            return Err(invalid(
                "directory Git reference does not name a user commit",
            ));
        }
        Ok(tree)
    }

    fn validate_git_child(
        &self,
        owner: &str,
        child: &str,
        branch: &str,
        tree: Option<&str>,
    ) -> Result<()> {
        let git = self
            .directory_git_repositories
            .get(child)
            .ok_or_else(|| invalid("directory Git child repository is missing"))?;
        if owner == child
            || git.repository_id != self.repository.repository_id
            || !git.branches.contains_key(branch)
        {
            return Err(invalid(
                "directory Git child reference escapes its repository",
            ));
        }
        if let Some(tree) = tree {
            self.git_committed_tree(child, branch, tree)?;
        }
        Ok(())
    }

    fn validate_git_index(
        &self,
        git: &DirectoryGitRepository,
        index: &DirectoryGitBranch,
        paths: &BTreeMap<&str, BTreeSet<&str>>,
    ) -> Result<()> {
        let mut normalized = BTreeSet::new();
        for (child, path) in &index.index_boundaries {
            name_policy::validate_path(path)?;
            let key = name_policy::path_key(path);
            let child_git = self
                .directory_git_repositories
                .get(child)
                .ok_or_else(|| invalid("staged boundary repository is missing"))?;
            if child == &git.directory_id
                || child_git.repository_id != git.repository_id
                || !paths
                    .get(child.as_str())
                    .is_some_and(|paths| paths.contains(key.as_str()))
                || !normalized.insert(key)
            {
                return Err(invalid(
                    "staged boundary identity or historical path differs",
                ));
            }
        }
        reject_nested_boundaries(&normalized)?;
        // Boundaries and child references can be stale until explicit stage-owned.
        // Never substitute the child's current HEAD or require current path equality.
        for (child, reference) in &index.index_children {
            self.validate_git_child(
                &git.directory_id,
                child,
                &reference.directory_branch_id,
                reference.tree_id.as_deref(),
            )?;
        }
        Ok(())
    }

    fn validate_git_commit(&self, tree: &DirectoryTree) -> Result<()> {
        let branch = &self.directory_branches[&tree.directory_branch_id];
        let git = self
            .directory_git_repositories
            .get(&branch.directory_id)
            .ok_or_else(|| invalid("directory commit has no initialized repository"))?;
        let commit = &self.commits[&tree.commit_id];
        if !git.branches.contains_key(&tree.directory_branch_id)
            || tree.sequence != commit.sequence
            || tree.sequence <= git.initialized_sequence
        {
            return Err(invalid("directory commit marker scope or sequence differs"));
        }
        let prefix =
            (!tree.path.is_empty()).then(|| format!("{}/", name_policy::path_key(&tree.path)));
        let projection = commit.entries.iter().filter(|entry| {
            prefix
                .as_ref()
                .is_none_or(|prefix| entry.lookup_key().starts_with(prefix))
        });
        if !projection.eq(tree.entries.iter()) {
            return Err(invalid(
                "directory commit namespace differs from its exact tree projection",
            ));
        }
        let parent_commit = tree
            .parent_tree_id
            .as_ref()
            .map(|parent| {
                let previous = self.git_committed_tree(
                    &branch.directory_id,
                    &tree.directory_branch_id,
                    parent,
                )?;
                if previous.sequence >= tree.sequence {
                    return Err(invalid(
                        "directory commit parent does not precede its child",
                    ));
                }
                Ok(previous.commit_id.as_str())
            })
            .transpose()?;
        let parents_match = match (parent_commit, commit.parent_commit_ids.as_slice()) {
            (None, []) => true,
            (Some(expected), [actual]) => expected == actual,
            _ => false,
        };
        if !parents_match {
            return Err(invalid(
                "directory commit parents differ from the committed tree lineage",
            ));
        }
        let entries = tree
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
            .map(|entry| (entry.entry_id.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let mut boundaries = BTreeSet::new();
        for (child, reference) in &tree.directory_refs {
            self.validate_git_child(
                &branch.directory_id,
                child,
                &reference.directory_branch_id,
                Some(&reference.tree_id),
            )?;
            let entry = entries
                .get(child.as_str())
                .ok_or_else(|| invalid("committed child boundary is absent from its snapshot"))?;
            boundaries.insert(entry.lookup_key().to_owned());
        }
        reject_nested_boundaries(&boundaries)?;
        for (child, reference) in &tree.directory_refs {
            let mount = entries[child.as_str()];
            let selected = &self.directory_trees[&reference.tree_id];
            // Both markers belong to this manifest's publication sequence
            // domain. Stage-child can pin only an already committed snapshot.
            // Raw working compositions can share a sequence and are not checked
            // here. Future cross-pool imports need an explicit sequence mapping.
            if selected.sequence >= tree.sequence {
                return Err(invalid(
                    "pinned child commit does not precede its parent commit",
                ));
            }
            Self::validate_git_child_projection(tree, mount, selected)?;
        }
        Ok(())
    }

    fn validate_git_child_projection(
        parent: &DirectoryTree,
        mount: &Entry,
        child: &DirectoryTree,
    ) -> Result<()> {
        // The marker projection above establishes canonical entry order. Direct
        // child ranges are disjoint, so their expansion checks do not rescan the
        // complete parent for every child repository.
        let prefix = format!("{}/", mount.lookup_key());
        let start = parent
            .entries
            .partition_point(|entry| entry.lookup_key() < prefix.as_str());
        let length = parent.entries[start..]
            .partition_point(|entry| entry.lookup_key().starts_with(&prefix));
        let actual = &parent.entries[start..start + length];
        if actual.len() != child.entries.len() {
            return Err(invalid(
                "committed child expansion differs from its pinned tree",
            ));
        }
        let depth = if child.path.is_empty() {
            0
        } else {
            child.path.split('/').count()
        };
        for (source, actual) in child.entries.iter().zip(actual) {
            let suffix = source
                .path()
                .splitn(depth + 1, '/')
                .last()
                .expect("invariant: validated child entry has a relative suffix");
            let mut expected = source.clone();
            expected.relocate(name_policy::PortablePath::parse(&format!(
                "{}/{suffix}",
                mount.path()
            ))?);
            if &expected != actual
                || (source.kind == NamespaceEntryKind::File
                    && parent.file_contexts.get(actual.lookup_key())
                        != child.file_contexts.get(source.lookup_key()))
            {
                return Err(invalid(
                    "committed child entries or contexts differ from the pinned tree",
                ));
            }
        }
        Ok(())
    }

    fn validate_git_tree_cycles(&self) -> Result<()> {
        let mut finished = BTreeSet::new();
        let mut active = BTreeSet::new();
        for tree in self
            .directory_trees
            .values()
            .filter(|tree| self.git_commit_marker(tree))
        {
            let mut pending = vec![(tree.tree_id.as_str(), false)];
            while let Some((id, exiting)) = pending.pop() {
                if exiting {
                    active.remove(id);
                    finished.insert(id);
                    continue;
                }
                if finished.contains(id) {
                    continue;
                }
                if !active.insert(id) {
                    return Err(invalid("committed directory references contain a cycle"));
                }
                pending.push((id, true));
                let tree = &self.directory_trees[id];
                if let Some(parent) = &tree.parent_tree_id {
                    pending.push((parent, false));
                }
                pending.extend(
                    tree.directory_refs
                        .values()
                        .map(|reference| (reference.tree_id.as_str(), false)),
                );
            }
        }
        Ok(())
    }
}

fn reject_nested_boundaries(paths: &BTreeSet<String>) -> Result<()> {
    for path in paths {
        let mut current = path.as_str();
        while let Some((parent, _)) = current.rsplit_once('/') {
            if paths.contains(parent) {
                return Err(invalid(
                    "directory Git boundaries are not independent direct children",
                ));
            }
            current = parent;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/history_package/directory_git_tests.rs"]
mod tests;

