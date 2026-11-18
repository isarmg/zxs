//! Current file branches borrow exact occurrences, never global historical IDs.
use super::*;
use crate::{Branch, BranchKind};

fn file_missing() -> Error {
    Error::new(
        ErrorCode::FileNotFound,
        "file branch source is absent from the current owning namespace",
    )
}

fn at_key<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries
        .binary_search_by(|entry| entry.lookup_key().cmp(key))
        .ok()
        .map(|index| &entries[index])
}

impl CurrentNamespaces {
    /// Live history derivation admits only a newly created pending helper.
    /// Its source is a current occurrence, not the source branch's frozen HEAD.
    pub(crate) fn history_merge_input_entries<'a>(
        &self,
        previous: &State,
        next: &'a State,
        branch: &Branch,
    ) -> Result<&'a [Entry]> {
        let commit = next.commits.get(&branch.head_commit_id).ok_or_else(stale)?;
        if commit.kind != "file_branch_pending"
            || previous.commits.contains_key(&commit.commit_id)
            || branch.generation != 1
            || next.file_branch_aliases.contains_key(&branch.branch_id)
        {
            return Err(unsupported());
        }
        let owner = self
            .new_file_branch_source(previous, next, branch, commit)?
            .ok_or_else(unsupported)?;
        self.validate_entries(Some(owner), &commit.entries)?;
        Ok(&commit.entries)
    }

    /// A new pending branch can choose an older body of a retained file. Its
    /// occurrence and every ancestor still come from one exact current owner.
    /// Reconstruct this proof from durable records during ordinary recovery too.
    pub(super) fn new_file_branch_source<'a>(
        &'a self,
        previous: &State,
        next: &State,
        branch: &Branch,
        commit: &Commit,
    ) -> Result<Option<&'a CurrentScope>> {
        if !previous.branches.contains_key(&branch.branch_id)
            && branch.file_id.is_some()
            && commit.kind == "file_metadata"
        {
            // A completed same-mutation history helper must enter through its
            // receipt/merge graph proof, never the ordinary synchronization path.
            return Err(unsupported());
        }
        if previous.branches.contains_key(&branch.branch_id)
            || branch.file_id.is_none()
            || !matches!(
                commit.kind.as_str(),
                "file_branch_pending" | "file_branch_root"
            )
        {
            return Ok(None);
        }
        let repository = previous
            .repositories
            .values()
            .find(|repository| repository.history_tree_id == branch.history_tree_id)
            .ok_or_else(stale)?;
        let owner = self.writable_scope(previous, &repository.main_branch_id)?;
        let file = branch
            .file_id
            .as_deref()
            .expect("invariant: file branch checked");
        let mut files = commit
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File);
        let entry = files
            .next()
            .filter(|entry| entry.file_id == file)
            .ok_or_else(file_missing)?;
        let original = at_key(&owner.entries, entry.lookup_key()).ok_or_else(file_missing)?;
        let version = next
            .versions
            .get(&entry.version_id)
            .ok_or_else(file_missing)?;
        if files.next().is_some()
            || branch.branch_kind != BranchKind::File
            || branch.state != BranchState::Active
            || branch.fork_commit_id != owner.head
            || branch.head_commit_id != commit.commit_id
            || commit.parent_commit_ids.len() != 1
            || commit.parent_commit_ids.first().map(String::as_str) != Some(owner.head.as_str())
            || commit.history_tree_id != branch.history_tree_id
            || original.kind != NamespaceEntryKind::File
            || original.file_id != file
            || original.entry_id != entry.entry_id
            || original.path() != entry.path()
            || version.file_id != file
            || version.history_tree_id != branch.history_tree_id
            || version.logical_size != entry.size
            || commit.namespace_deletions.is_some()
            || commit.sequence != next.sequence
        {
            return Err(file_missing());
        }
        let mut ancestors = BTreeSet::new();
        let mut key = entry.lookup_key();
        while let Some((parent, _)) = key.rsplit_once('/') {
            ancestors.insert(parent);
            key = parent;
        }
        if commit.entries.len() != ancestors.len() + 1
            || commit.entries.iter().any(|candidate| {
                candidate.kind == NamespaceEntryKind::Directory
                    && (!ancestors.contains(candidate.lookup_key())
                        || at_key(&owner.entries, candidate.lookup_key()) != Some(candidate))
            })
        {
            return Err(unsupported());
        }
        if branch.pending_file_versions != commit.pending_file_versions
            || commit
                .pending_file_versions
                .iter()
                .any(|(pending_file, version)| pending_file != file || version != &entry.version_id)
        {
            return Err(unsupported());
        }
        if let Some(node) = branch.file_node_id.as_deref()
            && crate::business::file_nodes::node_version(next, file, node)? != entry.version_id
        {
            return Err(unsupported());
        }
        if commit.kind == "file_branch_pending" {
            if commit.provenance.as_deref() != Some(entry.version_id.as_str())
                || commit.pending_file_versions.len() != 1
                || commit.pending_file_versions.get(file) != Some(&entry.version_id)
            {
                return Err(unsupported());
            }
        } else if entry != original || commit.provenance.as_deref() != Some(owner.head.as_str()) {
            return Err(file_missing());
        }
        // validate_entry_changes still rejects unavailable versions. Returning
        // this scope proves occurrence origin, not a body-read or write route.
        Ok(Some(owner))
    }

}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_file_branch_tests.rs"]
mod tests;
