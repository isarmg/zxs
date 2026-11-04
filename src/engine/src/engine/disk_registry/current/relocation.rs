//! Reconstruct relocation admission from the two authenticated namespace views.
//! No live-only plan or caller-supplied operation is needed during recovery.
use super::*;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum BodyIdentity<'a> {
    Directory(&'a str, &'a str, &'a str, u64),
    File(&'a str, &'a str, &'a str, u64),
}
impl<'a> From<&'a Entry> for BodyIdentity<'a> {
    fn from(entry: &'a Entry) -> Self {
        let Entry {
            entry_id,
            path: _,
            kind,
            file_id,
            version_id,
            size,
        } = entry;
        match kind {
            NamespaceEntryKind::Directory => Self::Directory(entry_id, file_id, version_id, *size),
            NamespaceEntryKind::File => Self::File(entry_id, file_id, version_id, *size),
        }
    }
}

fn same_identity(left: &Entry, right: &Entry) -> bool {
    left.entry_id == right.entry_id && left.kind == right.kind && left.file_id == right.file_id
}

fn descendant(key: &str, ancestor: &str) -> bool {
    key.strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn at_key<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries
        .binary_search_by(|entry| entry.lookup_key().cmp(key))
        .ok()
        .map(|index| &entries[index])
}

fn contains(entries: &[Entry], entry: &Entry) -> bool {
    at_key(entries, entry.lookup_key()) == Some(entry)
}

/// Every admitted moved occurrence consumes one visible source occurrence.
/// Reserve unchanged paths first: a retained alias cannot also authorize a copy.
/// Different versions of the same entry/file identity have separate source sets.
pub(super) struct VerifiedEntryChanges<'a> {
    relocated: BTreeSet<&'a str>,
    vacated: BTreeSet<&'a str>,
    synchronized: BTreeSet<&'a str>,
}
impl<'a> VerifiedEntryChanges<'a> {
    pub(super) fn between(before: &'a [Entry], after: &'a [Entry]) -> Self {
        Self::between_versions(before, after, None)
    }

    /// A moved and rewritten occurrence must consume its exact visible source.
    /// The new body is part of this publication, not an arbitrary historical
    /// version of the same file. Recovery derives this from authenticated States.
    pub(super) fn between_states(
        before: &'a [Entry],
        after: &'a [Entry],
        previous: &'a State,
        next: &'a State,
    ) -> Self {
        Self::between_versions(before, after, Some((previous, next)))
    }

    fn between_versions(
        before: &'a [Entry],
        after: &'a [Entry],
        states: Option<(&'a State, &'a State)>,
    ) -> Self {
        let next: BTreeMap<_, _> = after.iter().map(|e| (e.lookup_key(), e)).collect();
        let mut sources = BTreeMap::<BodyIdentity<'_>, Vec<&Entry>>::new();
        let mut vacated = BTreeSet::new();
        for entry in before {
            if next
                .get(entry.lookup_key())
                .is_some_and(|next| entry.path() == next.path() && same_identity(entry, next))
            {
                continue;
            }
            vacated.insert(entry.lookup_key());
            sources.entry(entry.into()).or_default().push(entry);
        }
        let previous: BTreeMap<_, _> = before.iter().map(|e| (e.lookup_key(), e)).collect();
        let mut change = Self {
            relocated: BTreeSet::new(),
            vacated,
            synchronized: BTreeSet::new(),
        };
        let mut rewritten = Vec::new();
        // Exact body relocations consume their source first. A rewritten alias
        // cannot steal that same occurrence and authorize an additional copy.
        for entry in after {
            if previous
                .get(entry.lookup_key())
                .is_some_and(|old| entry.path() == old.path() && same_identity(entry, old))
            {
                continue;
            }
            let identity = BodyIdentity::from(entry);
            if sources.get_mut(&identity).and_then(Vec::pop).is_some() {
                change.relocated.insert(entry.lookup_key());
            } else if entry.kind == NamespaceEntryKind::File {
                rewritten.push(entry);
            }
        }
        if let Some((previous, next)) = states {
            for entry in rewritten {
                if previous.versions.contains_key(&entry.version_id) {
                    continue;
                }
                let Some(version) = next.versions.get(&entry.version_id) else {
                    continue;
                };
                let [source] = version.source_version_ids.as_slice() else {
                    continue;
                };
                let Some(old) = previous.versions.get(source) else {
                    continue;
                };
                if version.version_sequence != next.sequence
                    || version.file_id != entry.file_id
                    || old.file_id != entry.file_id
                    || version.history_tree_id != old.history_tree_id
                    || version.logical_size != entry.size
                {
                    continue;
                }
                let identity =
                    BodyIdentity::File(&entry.entry_id, &entry.file_id, source, old.logical_size);
                if sources.get_mut(&identity).and_then(Vec::pop).is_some() {
                    change.relocated.insert(entry.lookup_key());
                }
            }
        }
        change
    }

    pub(super) fn preserves_occurrence(&self, entry: &Entry) -> bool {
        self.relocated.contains(entry.lookup_key())
            || self.synchronized.contains(entry.lookup_key())
    }

    pub(super) fn vacated(&self, key: &str) -> bool {
        self.vacated.contains(key)
    }

    /// A file branch can follow the namespace's new path on its next save.
    /// Only its exact selected occurrence and that occurrence's real ancestors
    /// may come from the already-validated owning repository namespace.
    pub(super) fn synchronize_file_branch(
        &mut self,
        state: &State,
        branch: &crate::Branch,
        owner: &[Entry],
        initial_owner: Option<&CurrentScope>,
        entries: &'a [Entry],
    ) -> Result<()> {
        let repository = state
            .repositories
            .values()
            .find(|repo| repo.history_tree_id == branch.history_tree_id)
            .ok_or_else(stale)?;
        let Some(file) = &branch.file_id else {
            return Ok(());
        };
        // A helper first created while freezing a rename's old directory view
        // still carries the old path. Its fork must bind that exact admitted
        // main head, not merely any globally retained entry/file identity.
        let initial = initial_owner
            .filter(|scope| branch.fork_commit_id == scope.head)
            .map(|scope| scope.entries.as_slice())
            .unwrap_or(&[]);
        let mut selected = BTreeSet::new();
        let mut parents = BTreeSet::new();
        for entry in entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File && &entry.file_id == file)
        {
            let current = contains(owner, entry)
                && state
                    .file_contexts
                    .get(&crate::business::context_key_from_key(
                        &repository.repository_id,
                        entry.lookup_key(),
                    ))
                    .is_some_and(|context| context.branch_id == branch.branch_id);
            if current || contains(initial, entry) {
                selected.insert(entry.lookup_key());
                let mut key = entry.lookup_key();
                while let Some((parent, _)) = key.rsplit_once('/') {
                    parents.insert(parent);
                    key = parent;
                }
            }
        }
        for entry in entries {
            if selected.contains(entry.lookup_key())
                || entry.kind == NamespaceEntryKind::Directory
                    && parents.contains(entry.lookup_key())
                    && (contains(owner, entry) || contains(initial, entry))
            {
                self.synchronized.insert(entry.lookup_key());
            }
        }
        Ok(())
    }

    /// Only the selected directory composition can collect newly admitted
    /// entries from its owning namespace. Inactive/frozen views keep their own
    /// identity fence; another scope's retained ID alone proves nothing here.
    pub(super) fn synchronize_directory(
        &mut self,
        state: &State,
        branch: &'a crate::DirectoryBranch,
        owner: &[Entry],
    ) -> Result<()> {
        let repository = state
            .repositories
            .get(&branch.repository_id)
            .ok_or_else(stale)?;
        if state.directory_selections.get(&branch.directory_id) != Some(&branch.directory_branch_id)
            || state.directory_tree_selections.get(&branch.directory_id)
                != Some(&branch.head_tree_id)
        {
            return Ok(());
        }
        let root = branch.directory_id == repository.repository_id && branch.path.is_empty();
        let path = name_policy::path_key(&branch.path);
        if !root
            && !at_key(owner, &path).is_some_and(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && entry.entry_id == branch.directory_id
                    && entry.path() == branch.path
            })
        {
            return Err(stale());
        }
        for entry in &branch.entries {
            if (root || descendant(entry.lookup_key(), &path)) && contains(owner, entry) {
                self.synchronized.insert(entry.lookup_key());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_namespace_relocation_tests.rs"]
mod tests;
