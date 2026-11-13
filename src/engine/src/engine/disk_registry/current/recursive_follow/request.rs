//! Reconstruct only the requested namespace/owner-path effects. Random commit,
//! node and parent-directory IDs are checked by the ordinary Current gates.
use super::*;
use crate::{DirectoryBatchKind, DirectoryNamespaceBatch};
use name_policy::PortablePath;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct FileRequest {
    repository_id: String,
    change: NamespaceOperation,
    original_file: Option<storage_contract::file_relocation::FileRelocation>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BatchRequest {
    repository_id: String,
    request: DirectoryNamespaceBatch,
}

struct Move {
    source: Entry,
    target: PortablePath,
    members: Vec<Entry>,
    kind: DirectoryBatchKind,
}

fn within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

impl CurrentNamespaces {
    pub(super) fn verify_follow_request(
        &self,
        previous: &State,
        next: &State,
        receipt: &OperationResult,
    ) -> Result<()> {
        let input = receipt.result["namespace_request"].clone();
        let (repository, moves, allow_parents) = match receipt.kind.as_str() {
            FILE_KIND | FILE_FOLLOW_KIND | FILE_ROUTE_KIND => {
                let request: FileRequest = json_codec::from_value(input)?;
                let moves = self.file_request_moves(previous, &request)?;
                (request.repository_id, moves, true)
            }
            BATCH_KIND | BATCH_FOLLOW_KIND | BATCH_ROUTE_KIND => {
                let request: BatchRequest = json_codec::from_value(input)?;
                let moves = self.batch_request_moves(previous, &request)?;
                (request.repository_id, moves, false)
            }
            _ => return Err(invalid()),
        };
        let repo = previous.repositories.get(&repository).ok_or_else(invalid)?;
        let branch = previous
            .branches
            .get(&repo.main_branch_id)
            .ok_or_else(invalid)?;
        self.require_branch_write(previous, &repo.main_branch_id)?;
        let before = self.branch_entries(&repo.main_branch_id, &branch.head_commit_id)?;
        let after_branch = next
            .branches
            .get(&repo.main_branch_id)
            .ok_or_else(invalid)?;
        let after_commit = next
            .commits
            .get(&after_branch.head_commit_id)
            .ok_or_else(invalid)?;
        let after = &after_commit.entries;
        if after_branch.head_commit_id != branch.head_commit_id {
            let principal = next
                .operation_principals
                .get(&receipt.operation_id)
                .ok_or_else(invalid)?;
            if previous.commits.contains_key(&after_commit.commit_id)
                || after_commit.parent_commit_ids != [branch.head_commit_id.clone()]
                || after_commit.history_tree_id != repo.history_tree_id
                || after_commit.operation_id != receipt.operation_id
                || after_commit.sequence != next.sequence
                || &after_commit.author != principal
                || after_commit.namespace_deletions.is_some()
                || branch.generation.checked_add(1) != Some(after_branch.generation)
            {
                return Err(invalid());
            }
        } else if matches!(
            receipt.kind.as_str(),
            FILE_KIND | FILE_FOLLOW_KIND | FILE_ROUTE_KIND
        ) {
            return Err(invalid());
        }
        let projected = project_entries(before, &moves)?;
        verify_entries(&projected, after, &moves, allow_parents)?;
        let locations = owner_paths(previous, &moves)?;
        let names = locations
            .iter()
            .map(|(file, path)| {
                (
                    file.clone(),
                    path.rsplit('/')
                        .next()
                        .expect("invariant: portable path has a leaf")
                        .to_owned(),
                )
            })
            .collect();
        if !super::super::backup_configuration::exact_upserts(
            &previous.file_locations,
            &next.file_locations,
            &locations,
        ) || !super::super::backup_configuration::exact_upserts(
            &previous.file_names,
            &next.file_names,
            &names,
        ) || !super::super::backup_configuration::exact_upserts(
            &previous.repositories,
            &next.repositories,
            &BTreeMap::new(),
        ) || !super::super::backup_configuration::exact_upserts(
            &previous.histories,
            &next.histories,
            &BTreeMap::new(),
        ) || receipt.history_tree_id.as_deref() != Some(&repo.history_tree_id)
            || receipt.branch_id.as_deref() != Some(&repo.main_branch_id)
            || receipt.commit_id.as_deref() != Some(&after_branch.head_commit_id)
        {
            return Err(invalid());
        }
        for id in changed_keys(&previous.branches, &next.branches) {
            for state in [previous, next] {
                if let Some(branch) = state.branches.get(&id)
                    && state
                        .histories
                        .get(&branch.history_tree_id)
                        .ok_or_else(invalid)?
                        .repository_id
                        != repository
                {
                    return Err(invalid());
                }
            }
        }
        for id in changed_keys(&previous.directory_branches, &next.directory_branches) {
            for state in [previous, next] {
                if let Some(branch) = state.directory_branches.get(&id)
                    && branch.repository_id != repository
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }

    fn file_request_moves(&self, state: &State, request: &FileRequest) -> Result<Vec<Move>> {
        let NamespaceOperation::Rename {
            source,
            destination,
        } = &request.change
        else {
            return Err(invalid());
        };
        let repo = state
            .repositories
            .get(&request.repository_id)
            .ok_or_else(invalid)?;
        let branch = state
            .branches
            .get(&repo.main_branch_id)
            .ok_or_else(invalid)?;
        let entries = self.branch_entries(&repo.main_branch_id, &branch.head_commit_id)?;
        if let Some(original) = &request.original_file {
            if original.repository_id.as_str() != request.repository_id
                || original.source.as_str() != source.as_str()
                || &original.destination != destination
            {
                return Err(invalid());
            }
            crate::business::file_relocation::require(
                state,
                original,
                &branch.head_commit_id,
                entries,
            )?;
        }
        let entry = entries
            .iter()
            .find(|entry| entry.lookup_key() == source.key())
            .ok_or_else(invalid)?;
        if within(destination.key(), source.key()) && destination.key() != source.key() {
            return Err(invalid());
        }
        let members = entries
            .iter()
            .filter(|entry| within(entry.lookup_key(), source.key()))
            .cloned()
            .collect();
        Ok(vec![Move {
            source: entry.clone(),
            target: destination.clone(),
            members,
            kind: DirectoryBatchKind::Rename,
        }])
    }

    fn batch_request_moves(&self, state: &State, request: &BatchRequest) -> Result<Vec<Move>> {
        if !matches!(
            request.request.kind,
            DirectoryBatchKind::Rename | DirectoryBatchKind::Move
        ) || request.request.items.is_empty()
        {
            return Err(invalid());
        }
        let mut moves = Vec::new();
        let mut mappings = BTreeSet::new();
        let mut active_paths = BTreeMap::<String, String>::new();
        for item in &request.request.items {
            let target = item.target.as_ref().ok_or_else(invalid)?;
            let source_path = PortablePath::parse(&item.source.path)?;
            let target_path = PortablePath::parse(&target.path)?;
            let mut captured = Vec::new();
            for (id, expected, path) in [
                (
                    &item.source.directory_branch_id,
                    &item.source.expected_tree_id,
                    source_path.as_str(),
                ),
                (
                    &target.directory_branch_id,
                    &target.expected_tree_id,
                    target_path.as_str(),
                ),
            ] {
                let branch = state.directory_branches.get(id).ok_or_else(invalid)?;
                if branch.repository_id != request.repository_id || &branch.head_tree_id != expected
                {
                    return Err(invalid());
                }
                crate::business::directory_git::require_owned_path(state, branch, path)?;
                let scope = self.batch_scope(state, id)?;
                if !active_paths.contains_key(id) {
                    let path = name_policy::path_key(&scope.branch().path);
                    if active_paths.values().any(|other| {
                        path.is_empty()
                            || other.is_empty()
                            || within(&path, other)
                            || within(other, &path)
                    }) {
                        return Err(invalid());
                    }
                    active_paths.insert(id.clone(), path);
                }
                captured.push(scope);
            }
            let source_scope = captured[0].branch();
            let target_scope = captured[1].branch();
            let source = source_scope
                .entries
                .iter()
                .find(|entry| {
                    entry.lookup_key() == source_path.key()
                        && entry.entry_id == item.source.entry_id
                })
                .ok_or_else(invalid)?;
            let scope_key = name_policy::path_key(&target_scope.path);
            if (!scope_key.is_empty()
                && (!within(target_path.key(), &scope_key) || target_path.key() == scope_key))
                || (request.request.kind == DirectoryBatchKind::Rename
                    && item.source.directory_branch_id != target.directory_branch_id)
                || (source.kind == NamespaceEntryKind::Directory
                    && within(target_path.key(), source.lookup_key())
                    && target_path.key() != source.lookup_key())
                || !mappings.insert((
                    item.source.directory_branch_id.clone(),
                    source.entry_id.clone(),
                    source.lookup_key().to_owned(),
                    target.directory_branch_id.clone(),
                    target_path.key().to_owned(),
                ))
            {
                return Err(invalid());
            }
            if moves.iter().any(|planned: &Move| {
                planned.source.lookup_key() != source.lookup_key()
                    && (within(planned.source.lookup_key(), source.lookup_key())
                        || within(source.lookup_key(), planned.source.lookup_key()))
            }) {
                return Err(invalid());
            }
            moves.push(Move {
                source: source.clone(),
                target: target_path,
                members: source_scope
                    .entries
                    .iter()
                    .filter(|entry| within(entry.lookup_key(), source.lookup_key()))
                    .cloned()
                    .collect(),
                kind: request.request.kind,
            });
        }
        Ok(moves)
    }
}

fn project_entries(before: &[Entry], moves: &[Move]) -> Result<BTreeMap<String, Entry>> {
    let mut expected: BTreeMap<_, _> = before
        .iter()
        .map(|entry| (entry.lookup_key().to_owned(), entry.clone()))
        .collect();
    let original = expected.clone();
    for change in moves {
        for entry in &change.members {
            if original.get(entry.lookup_key()) != Some(entry) {
                return Err(invalid());
            }
            expected.remove(entry.lookup_key());
        }
    }
    for change in moves {
        for entry in &change.members {
            let entry = crate::business::relocate_namespace_entry(
                entry,
                change.source.path(),
                &change.target,
            )?;
            if expected
                .insert(entry.lookup_key().to_owned(), entry)
                .is_some()
            {
                return Err(invalid());
            }
        }
    }
    // A file rename changes the shared container name, including occurrences
    // outside the selected directory. Only explicitly named file identities
    // contribute names; unrelated locations remain exact.
    let mut names = BTreeMap::new();
    for change in moves {
        if change.source.kind == NamespaceEntryKind::File {
            let file = change.source.file_id.parse()?;
            let name = name_policy::Name::parse(change.target.file_name())?;
            if let Some(previous) = names.insert(file, name.clone())
                && previous != name
            {
                return Err(invalid());
            }
        }
    }
    let mut entries = expected.into_values().collect::<Vec<_>>();
    crate::business::namespace::container_name::apply_file_names(&mut entries, &names)?;
    Ok(entries.into_iter().map(|entry| (entry.lookup_key().to_owned(), entry)).collect())
}

fn verify_entries(
    expected: &BTreeMap<String, Entry>,
    after: &[Entry],
    moves: &[Move],
    allow_parents: bool,
) -> Result<()> {
    let mut missing_parents = BTreeMap::new();
    for change in moves {
        let mut path = change.target.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            let key = name_policy::path_key(parent);
            match expected.get(&key) {
                Some(entry) if entry.kind == NamespaceEntryKind::Directory => (),
                None if allow_parents => {
                    missing_parents.insert(key, parent.to_owned());
                }
                _ => return Err(invalid()),
            }
            path = parent;
        }
    }
    if after.len() != expected.len() + missing_parents.len() {
        return Err(invalid());
    }
    for entry in after {
        if let Some(expected) = expected.get(entry.lookup_key()) {
            if entry != expected {
                return Err(invalid());
            }
        } else if missing_parents.get(entry.lookup_key()).map(String::as_str) != Some(entry.path())
            || entry.kind != NamespaceEntryKind::Directory
            || !entry.file_id.is_empty()
            || !entry.version_id.is_empty()
            || entry.size != 0
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn owner_paths(previous: &State, moves: &[Move]) -> Result<BTreeMap<String, String>> {
    let mut paths = BTreeMap::new();
    for change in moves {
        for entry in change
            .members
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            let before = previous
                .file_locations
                .get(&entry.file_id)
                .cloned()
                .unwrap_or_else(|| entry.path().to_owned());
            let next = crate::business::namespace_location(
                before,
                entry,
                change.source.path(),
                change.target.as_str(),
                change.kind,
                change.source.kind == NamespaceEntryKind::Directory,
            );
            if let Some(other) = paths.insert(entry.file_id.clone(), next.clone())
                && name_policy::path_key(&other) != name_policy::path_key(&next)
            {
                return Err(invalid());
            }
        }
    }
    Ok(paths)
}
