//! Rename may directly leave a directory repository only before that file has
//! appeared in one of its explicit commits. Working saves and indexes do not
//! count; a historical export needs an independent full-history copy instead.
use super::*;

pub(crate) struct RenameOwnership<'a> {
    before: BTreeMap<&'a str, &'a str>,
    after: BTreeMap<&'a str, &'a str>,
    departures: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

impl<'a> RenameOwnership<'a> {
    pub(crate) fn new(
        state: &State,
        repository: &'a str,
        before: &'a [Entry],
        after: &'a [Entry],
    ) -> Self {
        Self {
            before: owners(state, repository, before),
            after: owners(state, repository, after),
            departures: BTreeMap::new(),
        }
    }

    pub(crate) fn track(&mut self, source: &'a Entry, target: &Entry) {
        let Some(owner) = owner_at(&self.before, source.lookup_key()) else {
            return;
        };
        if owner_at(&self.after, target.lookup_key()) != Some(owner) {
            self.departures
                .entry(owner)
                .or_default()
                .insert(&source.file_id);
        }
    }

    pub(crate) fn requires_history_copy(&self, state: &State) -> Result<bool> {
        has_committed_departure(state, &self.departures)
    }

    pub(crate) fn require_direct(&self, state: &State) -> Result<()> {
        if self.requires_history_copy(state)? {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "renaming a historically committed file out of its directory repository requires a full-history copy; source and destination are unchanged",
            ));
        }
        Ok(())
    }
}

pub(super) fn owners<'a>(
    state: &State,
    repository: &'a str,
    entries: &'a [Entry],
) -> BTreeMap<&'a str, &'a str> {
    let mut result = entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && state
                    .directory_git_repositories
                    .contains_key(&entry.entry_id)
        })
        .map(|entry| (entry.lookup_key(), entry.entry_id.as_str()))
        .collect::<BTreeMap<_, _>>();
    if state.directory_git_repositories.contains_key(repository) {
        result.insert("", repository);
    }
    result
}

pub(super) fn owner_at<'a>(owners: &BTreeMap<&str, &'a str>, key: &str) -> Option<&'a str> {
    let parent = key.rsplit_once('/').map_or("", |(parent, _)| parent);
    namespace::closest_directory_repository(parent, |key| owners.get(key).copied())
}

fn owned_files<'a>(
    tree: &'a DirectoryTree,
    files: &'a BTreeSet<&str>,
) -> impl Iterator<Item = &'a str> {
    let children = tree
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && tree.directory_refs.contains_key(&entry.entry_id)
        })
        .map(Entry::lookup_key)
        .collect::<BTreeSet<_>>();
    tree.entries
        .iter()
        .filter(move |entry| {
            if entry.kind != NamespaceEntryKind::File || !files.contains(entry.file_id.as_str()) {
                return false;
            }
            let mut key = entry.lookup_key();
            while let Some((parent, _)) = key.rsplit_once('/') {
                if children.contains(parent) {
                    return false;
                }
                key = parent;
            }
            true
        })
        .map(|entry| entry.file_id.as_str())
}

pub(super) fn contains_owned_file(tree: &DirectoryTree, files: &BTreeSet<&str>) -> bool {
    owned_files(tree, files).next().is_some()
}

pub(crate) fn committed_files(state: &State, files: &BTreeSet<&str>) -> BTreeSet<String> {
    state
        .directory_trees
        .values()
        .filter(|tree| {
            storage_contract::directory_staging::directory_tree_is_committed(state, tree)
        })
        .flat_map(|tree| owned_files(tree, files))
        .map(str::to_owned)
        .collect()
}

pub(super) fn has_committed_departure(
    state: &State,
    departures: &BTreeMap<&str, BTreeSet<&str>>,
) -> Result<bool> {
    if departures.is_empty() {
        return Ok(false);
    }
    let mut remaining = 1_000_000usize;
    for tree in state.directory_trees.values() {
        let branch = &state.directory_branches[&tree.directory_branch_id];
        let count = 1 + tree.entries.len();
        remaining = remaining.checked_sub(count).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "directory ownership history scan budget exceeded",
            )
        })?;
        if let Some(files) = departures.get(branch.directory_id.as_str())
            && storage_contract::directory_staging::directory_tree_is_committed(state, tree)
            && contains_owned_file(tree, files)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
