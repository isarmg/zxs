//! New helpers can follow an owner admitted earlier in the same publication.
//! Exact current occurrences and permanent path facts prove this during
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
            || commit.kind != "file_branch_root"
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
        Ok(Some(owner))
    }
}
