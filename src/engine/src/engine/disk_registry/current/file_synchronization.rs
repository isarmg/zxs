//! New helpers can follow an owner admitted earlier in the same publication.
//! Exact current occurrences and the immutable path graph prove this during
//! recovery too; a global file ID or an arbitrary metadata node does not.
use super::*;
use crate::{Branch, BranchKind};

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "new file helper differs from its admitted owner or path lineage",
    )
}

fn at_key<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries
        .binary_search_by(|entry| entry.lookup_key().cmp(key))
        .ok()
        .map(|index| &entries[index])
}

/// A helper snapshot contains just its one selected occurrence and its exact
/// current ancestors, not unrelated or frozen occurrences sharing an identity.
fn selected_entry<'a>(
    next: &State,
    repository: &str,
    branch: &Branch,
    commit: &'a Commit,
    owner: &CurrentScope,
) -> Option<&'a Entry> {
    let file = branch.file_id.as_deref()?;
    let mut files = commit
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File);
    let entry = files.next()?;
    if files.next().is_some()
        || entry.file_id != file
        || at_key(&owner.entries, entry.lookup_key()) != Some(entry)
        || !next
            .file_contexts
            .get(&crate::business::context_key_from_key(
                repository,
                entry.lookup_key(),
            ))
            .is_some_and(|context| {
                context.branch_id == branch.branch_id && context.node_id == branch.file_node_id
            })
    {
        return None;
    }
    let mut ancestors = BTreeSet::new();
    let mut key = entry.lookup_key();
    while let Some((parent, _)) = key.rsplit_once('/') {
        ancestors.insert(parent);
        key = parent;
    }
    (commit.entries.len() == ancestors.len() + 1
        && commit.entries.iter().all(|candidate| {
            candidate == entry
                || candidate.kind == NamespaceEntryKind::Directory
                    && ancestors.contains(candidate.lookup_key())
                    && at_key(&owner.entries, candidate.lookup_key()) == Some(candidate)
        }))
    .then_some(entry)
}

impl CurrentNamespaces {
    pub(super) fn new_synchronized_file_branch_source<'a>(
        &self,
        previous: &State,
        next: &State,
        branch: &Branch,
        commit: &Commit,
        published: &'a Self,
    ) -> Result<Option<&'a CurrentScope>> {
        if previous.branches.contains_key(&branch.branch_id)
            || branch.file_id.is_none()
            || !matches!(commit.kind.as_str(), "file_branch_root" | "file_metadata")
        {
            return Ok(None);
        }
        let repository = next
            .repositories
            .values()
            .find(|repository| repository.history_tree_id == branch.history_tree_id)
            .ok_or_else(stale)?;
        let owner = published.writable_scope(next, &repository.main_branch_id)?;
        let Some(entry) = selected_entry(next, &repository.repository_id, branch, commit, owner)
        else {
            return Ok(None);
        };
        let initial = self.writable_scope(previous, &repository.main_branch_id)?;
        let version = next.versions.get(&entry.version_id).ok_or_else(invalid)?;
        if version.file_id != entry.file_id
            || version.history_tree_id != branch.history_tree_id
            || version.logical_size != entry.size
            || branch.branch_kind != BranchKind::File
            || branch.state != BranchState::Active
            || next.file_branch_aliases.contains_key(&branch.branch_id)
            || previous.commits.contains_key(&commit.commit_id)
            || commit.history_tree_id != branch.history_tree_id
            || commit.sequence != next.sequence
            || commit.namespace_deletions.is_some()
            || branch.head_commit_id != commit.commit_id
            || branch.pending_file_versions != commit.pending_file_versions
            || commit
                .pending_file_versions
                .iter()
                .any(|(file, version)| file != &entry.file_id || version != &entry.version_id)
        {
            return Err(invalid());
        }
        if commit.kind == "file_branch_root" {
            // Copies allocate their helper before publishing the new main HEAD.
            // Their old fork is ancestry, not the source of the new occurrence.
            if branch.generation != 1
                || branch.file_node_id.is_some()
                || commit.parent_commit_ids.as_slice() != [branch.fork_commit_id.as_str()]
                || commit.provenance.as_deref() != Some(branch.fork_commit_id.as_str())
            {
                return Err(invalid());
            }
            if !previous.versions.contains_key(&entry.version_id) {
                let publication = next.commits.get(&owner.head).ok_or_else(stale)?;
                if version.version_sequence != next.sequence
                    || commit.operation_id != publication.operation_id
                    || commit.author != publication.author
                {
                    return Err(invalid());
                }
            }
            if branch.fork_commit_id != initial.head && branch.fork_commit_id != owner.head {
                // A current selected directory can still name an older source
                // commit. Only a newly allocated file/body may use that ancestry;
                // its visible occurrence was already admitted in the new owner.
                let fork = next
                    .commits
                    .get(&branch.fork_commit_id)
                    .ok_or_else(invalid)?;
                if self.known_entries.contains(&entry.entry_id)
                    || self.known_files.contains(&entry.file_id)
                    || previous.versions.contains_key(&entry.version_id)
                    || version.version_sequence != next.sequence
                    || fork.history_tree_id != branch.history_tree_id
                {
                    return Err(invalid());
                }
            }
        } else {
            self.verify_new_path_helper(previous, next, branch, commit, entry)?;
        }
        Ok(Some(owner))
    }

    fn verify_new_path_helper(
        &self,
        previous: &State,
        next: &State,
        branch: &Branch,
        head: &Commit,
        entry: &Entry,
    ) -> Result<()> {
        let node = branch
            .file_node_id
            .as_ref()
            .and_then(|id| next.file_path_nodes.get(id))
            .ok_or_else(invalid)?;
        let [metadata_id] = head.parent_commit_ids.as_slice() else {
            return Err(invalid());
        };
        let metadata = next.commits.get(metadata_id).ok_or_else(invalid)?;
        let [root_id] = metadata.parent_commit_ids.as_slice() else {
            return Err(invalid());
        };
        let root = next.commits.get(root_id).ok_or_else(invalid)?;
        if branch.generation != 3
            || root.kind != "file_branch_root"
            || metadata.kind != "file_metadata"
            || previous.commits.contains_key(metadata_id)
            || previous.commits.contains_key(root_id)
            || previous.file_path_nodes.contains_key(&node.node_id)
            || node.branch_id != branch.branch_id
            || node.history_tree_id != branch.history_tree_id
            || node.file_id != entry.file_id
            || node.version_id != entry.version_id
            || node.path != entry.path()
            || node.sequence != next.sequence
            || node.operation_id != head.operation_id
            || node.author != head.author
            || head.provenance.as_deref() != Some(node.node_id.as_str())
        {
            return Err(invalid());
        }
        // The two metadata commits advance the body/node and then its path.
        // Apart from the new snapshot identity and path, relocation changes none
        // of the authenticated metadata publication's fields.
        let mut relocated = metadata.clone();
        relocated.commit_id = head.commit_id.clone();
        relocated.tree_id = head.tree_id.clone();
        relocated.parent_commit_ids = vec![metadata_id.clone()];
        relocated.entries = head.entries.clone();
        if relocated != *head {
            return Err(invalid());
        }
        let mut source = branch.clone();
        source.head_commit_id = root_id.clone();
        source.file_node_id = None;
        source.pending_file_versions = root.pending_file_versions.clone();
        source.generation = 1;
        let initial = self
            .new_file_branch_source(previous, next, &source, root)?
            .ok_or_else(invalid)?;
        let original = at_key(
            &initial.entries,
            &name_policy::path_key(&node.previous_path),
        )
        .filter(|original| {
            original.kind == NamespaceEntryKind::File
                && original.entry_id == entry.entry_id
                && original.file_id == entry.file_id
                && original.path() == node.previous_path
                && at_key(&root.entries, original.lookup_key()) == Some(*original)
        })
        .ok_or_else(invalid)?;
        let history = previous
            .histories
            .get(&branch.history_tree_id)
            .ok_or_else(stale)?;
        let original_node = previous
            .file_contexts
            .get(&crate::business::context_key_from_key(
                &history.repository_id,
                original.lookup_key(),
            ))
            .and_then(|context| context.node_id.as_deref())
            .unwrap_or(original.version_id.as_str());
        if crate::business::file_nodes::node_version(previous, &entry.file_id, original_node)?
            != original.version_id
        {
            return Err(invalid());
        }
        let parent = if original.version_id == entry.version_id {
            original_node
        } else {
            let version = next.versions.get(&entry.version_id).ok_or_else(invalid)?;
            if previous.versions.contains_key(&entry.version_id)
                || version.version_sequence != next.sequence
                || version.file_id != entry.file_id
                || version.history_tree_id != branch.history_tree_id
                || version.logical_size != entry.size
                || version.source_version_ids.as_slice() != [original.version_id.as_str()]
                || !metadata.pending_file_versions.is_empty()
            {
                return Err(invalid());
            }
            entry.version_id.as_str()
        };
        let mut expected = root.entries.clone();
        for file in expected
            .iter_mut()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            file.version_id = entry.version_id.clone();
            file.size = entry.size;
        }
        if node.parent_node_ids.as_slice() != [parent]
            || metadata.entries != expected
            || (original.version_id == entry.version_id
                && metadata.pending_file_versions != root.pending_file_versions)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
