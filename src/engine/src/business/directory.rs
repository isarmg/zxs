//! Frozen directory compositions over public file branches and immutable nodes.
use super::*;
use json_codec::json;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn required<'a, T>(value: Option<&'a T>, message: &str) -> Result<&'a T> {
    value.ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, message))
}

fn selected_tree<'a>(state: &'a State, directory: &str) -> Result<(&'a str, &'a DirectoryTree)> {
    let branch_id = required(
        state.directory_selections.get(directory),
        "directory selection missing",
    )?;
    let branch = required(
        state.directory_branches.get(branch_id),
        "selected directory branch missing",
    )?;
    let tree_id = state
        .directory_tree_selections
        .get(directory)
        .unwrap_or(&branch.head_tree_id);
    let tree = required(
        state.directory_trees.get(tree_id),
        "selected directory tree missing",
    )?;
    required(
        state.commits.get(&tree.commit_id),
        "selected directory tree commit missing",
    )?;
    Ok((branch_id, tree))
}

pub(crate) fn context_key_from_key(repository: &str, key: &str) -> String {
    format!("{repository}/{key}")
}
pub(crate) fn context_at_key(state: &State, repository: &str, key: &str) -> Option<FileContext> {
    if let Some(context) = state
        .file_contexts
        .get(&context_key_from_key(repository, key))
    {
        return Some(context.clone());
    }
    let repo = state.repositories.get(repository)?;
    let commit = state
        .commits
        .get(&state.branches.get(&repo.main_branch_id)?.head_commit_id)?;
    let entry = namespace::find_unordered_key(&commit.entries, key)
        .filter(|e| e.kind == NamespaceEntryKind::File)?;
    Some(context_for_entry(state, repo, entry))
}

// A freshly written namespace has not registered a directory composition yet.
// Bootstrap its live context here; frozen DirectoryBranch/Tree readers never do.
fn context_for_entry(state: &State, repo: &Repository, entry: &Entry) -> FileContext {
    let branch = state
        .file_selections
        .get(&entry.file_id)
        .cloned()
        .unwrap_or_else(|| repo.main_branch_id.clone());
    FileContext {
        node_id: state
            .branches
            .get(&branch)
            .and_then(|b| b.file_node_id.clone()),
        selection_generation: 0,
        branch_id: branch,
        inherited_from: state.file_write_origins.get(&entry.file_id).cloned(),
    }
}
pub(crate) fn branch_context_at_key(
    state: &State,
    namespace: &str,
    key: &str,
) -> Option<FileContext> {
    let branch = state.branches.get(namespace)?;
    if let Some(repo) = repository::main_for_branch(state, namespace) {
        context_at_key(state, &repo.repository_id, key)
    } else {
        Some(FileContext {
            node_id: branch.file_node_id.clone(),
            selection_generation: 0,
            branch_id: branch.branch_id.clone(),
            inherited_from: None,
        })
    }
}

struct DirectoryScopeView<'a> {
    directory_id: &'a str,
    path: &'a str,
    key: Cow<'a, str>,
    commit: &'a Commit,
    entries: &'a [Entry],
}
fn scope_view<'a>(
    state: &'a State,
    repository: &str,
    path: &str,
) -> Result<DirectoryScopeView<'a>> {
    if !path.is_empty() {
        name_policy::validate_lookup_path(path)?;
    }
    let repo = state.repositories.get(repository).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RepositoryNotFound,
            "directory repository missing",
        )
    })?;
    scope_view_for_key(state, repo, Cow::Owned(name_policy::path_key(path)))
}
fn scope_view_for_key<'a>(
    state: &'a State,
    repo: &'a Repository,
    key: Cow<'a, str>,
) -> Result<DirectoryScopeView<'a>> {
    let branch = required(
        state.branches.get(&repo.main_branch_id),
        "repository main branch missing",
    )?;
    let commit = required(
        state.commits.get(&branch.head_commit_id),
        "repository head commit missing",
    )?;
    let (directory_id, path) = if key.is_empty() {
        (repo.repository_id.as_str(), "")
    } else {
        let entry = commit
            .entries
            .iter()
            .find(|entry| {
                entry.kind == NamespaceEntryKind::Directory && entry.lookup_key() == key.as_ref()
            })
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::DirectoryNotFound,
                    "directory missing",
                )
            })?;
        (entry.entry_id.as_str(), entry.path())
    };
    Ok(DirectoryScopeView {
        directory_id,
        path,
        key,
        commit,
        entries: &commit.entries,
    })
}
struct DirectorySnapshotSource {
    directory_id: String,
    path: String,
    current: Arc<Commit>,
    entries: Vec<Entry>,
}
impl DirectorySnapshotSource {
    fn capture(state: &State, view: DirectoryScopeView<'_>) -> Self {
        let entries = view
            .entries
            .iter()
            .filter(|entry| {
                view.key.is_empty() || namespace::is_descendant(entry.lookup_key(), &view.key)
            })
            .cloned()
            .collect();
        Self {
            directory_id: view.directory_id.into(),
            path: view.path.into(),
            current: state
                .commits
                .get_shared(&view.commit.commit_id)
                .expect("invariant: scope view borrows an existing commit"),
            entries,
        }
    }

    /// Resolve identity and contents together from the admitted owning main.
    /// The immutable commit supplies provenance, never the projected entries.
    fn current_view<'a>(
        engine: &'a Engine,
        repository: &str,
        key: Cow<'a, str>,
    ) -> Result<DirectoryScopeView<'a>> {
        let source = Self::current_read_view(engine, repository, key)?;
        let repo = &engine.state.repositories[repository];
        let branch = &engine.state.branches[&repo.main_branch_id];
        if let Some(view) = engine.admitted_current_namespaces()? {
            view.require_branch_write(&engine.state, &repo.main_branch_id)?;
        } else if branch.state != BranchState::Active {
            return Err(Error::new(
                storage_contract::ErrorCode::BranchNotActive,
                "directory namespace is not writable",
            ));
        }
        Ok(source)
    }

    fn current_read_view<'a>(
        engine: &'a Engine,
        repository: &str,
        key: Cow<'a, str>,
    ) -> Result<DirectoryScopeView<'a>> {
        let repo = engine.state.repositories.get(repository).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::RepositoryNotFound,
                "repository missing",
            )
        })?;
        let branch = required(
            engine.state.branches.get(&repo.main_branch_id),
            "repository main branch missing",
        )?;
        let tree = engine.namespace_tree(&repo.main_branch_id)?;
        let (directory_id, path) = if key.is_empty() {
            (repo.repository_id.as_str(), "")
        } else {
            let entry = tree
                .get_key(&key)
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::DirectoryNotFound,
                        "directory missing from the current namespace",
                    )
                })?;
            (entry.entry_id.as_str(), entry.path())
        };
        let entries = current_scope_entries(tree.entries(), &key);
        Ok(DirectoryScopeView {
            directory_id,
            path,
            key,
            commit: engine
                .state
                .commits
                .get(&branch.head_commit_id)
                .expect("invariant: current namespace borrows an existing commit"),
            entries,
        })
    }
}

fn current_scope_entries<'a>(entries: &'a [Entry], key: &str) -> &'a [Entry] {
    if key.is_empty() {
        return entries;
    }
    let prefix = format!("{key}/");
    let start = entries.partition_point(|entry| entry.lookup_key() < prefix.as_str());
    let descendants = &entries[start..];
    let end = descendants.partition_point(|entry| entry.lookup_key().starts_with(&prefix));
    &descendants[..end]
}

pub(crate) fn scope(
    state: &State,
    repository: &str,
    path: &str,
) -> Result<(String, Vec<Entry>, String)> {
    let view = scope_view(state, repository, path)?;
    let entries = view
        .commit
        .entries
        .iter()
        .filter(|entry| {
            view.key.is_empty() || namespace::is_descendant(entry.lookup_key(), &view.key)
        })
        .cloned()
        .collect();
    Ok((
        view.directory_id.to_owned(),
        entries,
        view.commit.commit_id.clone(),
    ))
}

pub(crate) fn rebase_path(path: &str, from: &str, to: &str) -> Result<String> {
    let depth = if from.is_empty() {
        0
    } else {
        from.split('/').count()
    };
    let suffix = path.split('/').skip(depth).collect::<Vec<_>>().join("/");
    if suffix.is_empty() {
        return Err(Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "directory snapshot contains its own root",
        ));
    }
    let path = if to.is_empty() {
        suffix
    } else {
        format!("{to}/{suffix}")
    };
    name_policy::validate_path(&path)?;
    Ok(path)
}

pub(crate) fn rebase_entries(entries: &[Entry], from: &str, to: &str) -> Result<Vec<Entry>> {
    let depth = if from.is_empty() {
        0
    } else {
        from.split('/').count()
    };
    let target = if to.is_empty() {
        None
    } else {
        Some(name_policy::PortablePath::parse(to)?)
    };
    entries
        .iter()
        .map(|entry| {
            let path = if let Some(target) = &target {
                entry.location().rebase(depth, target)?
            } else {
                entry.location().strip_components(depth).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "directory snapshot contains its own root",
                    )
                })?
            };
            let mut entry = entry.clone();
            entry.relocate(path);
            Ok(entry)
        })
        .collect()
}

fn snapshot(state: &mut State, repository: &str, path: &str) -> Result<String> {
    let source = DirectorySnapshotSource::capture(state, scope_view(state, repository, path)?);
    snapshot_source(state, repository, source)
}

/// Freeze selected children from the same current owner before freezing their
/// parent. Keeping an old child-tree reference would retain hidden occurrences
/// even when the new parent's own entries have already been projected.
fn snapshot_current_source(
    state: &mut State,
    repository: &str,
    source: DirectorySnapshotSource,
) -> Result<String> {
    let mut selected = source
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && state.directory_selections.contains_key(&entry.entry_id)
        })
        .collect::<Vec<_>>();
    selected.sort_by_key(|entry| std::cmp::Reverse(entry.path().split('/').count()));
    for directory in selected {
        snapshot_source(
            state,
            repository,
            DirectorySnapshotSource {
                directory_id: directory.entry_id.clone(),
                path: directory.path().into(),
                current: Arc::clone(&source.current),
                entries: current_scope_entries(&source.entries, directory.lookup_key()).to_vec(),
            },
        )?;
    }
    snapshot_source(state, repository, source)
}

fn snapshot_source(
    state: &mut State,
    repository: &str,
    source: DirectorySnapshotSource,
) -> Result<String> {
    snapshot_source_with_ids(state, repository, source, &mut |prefix| Ok(new_id(prefix)?))
}

fn snapshot_source_with_ids(
    state: &mut State,
    repository: &str,
    source: DirectorySnapshotSource,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    let DirectorySnapshotSource {
        directory_id,
        path,
        current,
        entries,
    } = source;
    let commit_id = current.commit_id.clone();
    let mut files = BTreeMap::new();
    for e in entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
    {
        files.insert(
            e.file_id.clone(),
            file::ensure_shared_file_branch_with_ids(state, &current, e, ids)?,
        );
    }
    let file_contexts = entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .map(|e| {
            let branch_id =
                required(files.get(&e.file_id), "snapshot file branch missing")?.clone();
            let context =
                context_at_key(state, repository, e.lookup_key()).unwrap_or(FileContext {
                    node_id: None,
                    selection_generation: 0,
                    branch_id,
                    inherited_from: None,
                });
            state.file_contexts.insert(
                context_key_from_key(repository, e.lookup_key()),
                context.clone(),
            );
            Ok((e.lookup_key().to_owned(), context))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let inherited_files = state
        .file_write_origins
        .iter()
        .filter(|(id, _)| files.contains_key(*id))
        .map(|(id, origin)| (id.clone(), origin.clone()))
        .collect();
    let mut directory_refs = BTreeMap::new();
    for entry in entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::Directory)
    {
        if state.directory_selections.contains_key(&entry.entry_id) {
            let (branch, tree) = selected_tree(state, &entry.entry_id)?;
            directory_refs.insert(
                entry.entry_id.clone(),
                DirectoryReference {
                    directory_branch_id: branch.into(),
                    tree_id: tree.tree_id.clone(),
                },
            );
        } else if state
            .directory_tree_selections
            .contains_key(&entry.entry_id)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory tree selection has no branch",
            ));
        }
    }
    let previous = state.directory_selections.get(&directory_id).cloned();
    let parent_tree_id = if previous.is_some() {
        Some(selected_tree(state, &directory_id)?.1.tree_id.clone())
    } else {
        if state.directory_tree_selections.contains_key(&directory_id) {
            return Err(Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory tree selection has no branch",
            ));
        }
        None
    };
    let id = match previous {
        Some(id) => id,
        None => ids("dbr")?,
    };
    // One operation can touch several siblings. Publish a single consolidated
    // node for this scope, and no node for a byte-for-byte identical view.
    if let Some(old) = parent_tree_id
        .as_ref()
        .and_then(|id| state.directory_trees.get(id))
        && old.path == path
        && old.entries == entries
        && old.file_contexts == file_contexts
        && old.file_branches == files
        && old.inherited_files == inherited_files
        && old.directory_refs == directory_refs
    {
        return Ok(id);
    }
    let tree_id = ids("dtree")?;
    if state.directory_trees.contains_key(&tree_id) {
        return Err(Error::new(
            ErrorCode::StateInvalid,
            "directory snapshot identity already exists",
        ));
    }
    state.directory_trees.insert(
        tree_id.clone(),
        DirectoryTree {
            tree_id: tree_id.clone(),
            directory_branch_id: id.clone(),
            parent_tree_id,
            commit_id: commit_id.clone(),
            path: path.clone(),
            entries: entries.clone(),
            file_contexts: file_contexts.clone(),
            file_branches: files.clone(),
            inherited_files: inherited_files.clone(),
            directory_refs: directory_refs.clone(),
            sequence: state.sequence,
        },
    );
    if let Some(target) = state.directory_branches.get_mut(&id) {
        target.head_tree_id = tree_id.clone();
        target.commit_id = commit_id;
        target.path = path;
        target.entries = entries;
        target.file_contexts = file_contexts;
        target.file_branches = files;
        target.inherited_files = inherited_files;
        target.directory_refs = directory_refs;
        target.materialized = true;
        target.sequence = state.sequence;
    } else {
        state.directory_branches.insert(
            id.clone(),
            DirectoryBranch {
                directory_branch_id: id.clone(),
                repository_id: repository.into(),
                directory_id: directory_id.clone(),
                path,
                name: "main".into(),
                parent_branch_id: None,
                head_tree_id: tree_id.clone(),
                commit_id,
                entries,
                file_contexts,
                file_branches: files,
                inherited_files,
                directory_refs,
                materialized: true,
                sequence: state.sequence,
            },
        );
    }
    state
        .directory_selections
        .insert(directory_id.clone(), id.clone());
    state
        .directory_tree_selections
        .insert(directory_id, tree_id);
    Ok(id)
}

/// Preserve inherited file-write intent in a newly published namespace.
pub(crate) fn snapshot_pending(state: &State, entries: &[Entry]) -> BTreeMap<String, String> {
    entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .filter_map(|e| {
            let context = state
                .repositories
                .values()
                .find(|r| {
                    state
                        .versions
                        .get(&e.version_id)
                        .is_some_and(|v| v.history_tree_id == r.history_tree_id)
                })
                .and_then(|r| context_at_key(state, &r.repository_id, e.lookup_key()));
            context.and_then(|c| {
                if c.inherited_from.is_some()
                    || state
                        .branches
                        .get(&c.branch_id)
                        .is_some_and(|b| b.pending_file_versions.contains_key(&e.file_id))
                {
                    Some((e.file_id.clone(), e.version_id.clone()))
                } else {
                    None
                }
            })
        })
        .collect()
}

/// Capture a manually initialized repository's working composition, including
/// plain subdirectories. These internal snapshots do not initialize child repos.
pub(super) fn initialize_git_working_scope(
    state: &mut State,
    repository: &str,
    path: &str,
    view: Option<&crate::engine::CurrentNamespaces>,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    let repo = state
        .repositories
        .get(repository)
        .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "repository missing"))?;
    let main = &state.branches[&repo.main_branch_id];
    let current = state
        .commits
        .get_shared(&main.head_commit_id)
        .expect("admitted namespace head");
    let entries = match view {
        Some(view) => {
            view.require_branch_write(state, &repo.main_branch_id)?;
            view.branch_entries(&repo.main_branch_id, &main.head_commit_id)?
                .to_vec()
        }
        None => current.entries.clone(),
    };
    let key = name_policy::path_key(path);
    let (directory_id, path) = if path.is_empty() {
        (repository.to_owned(), String::new())
    } else {
        let entry = entries
            .iter()
            .find(|entry| entry.kind == NamespaceEntryKind::Directory && entry.lookup_key() == key)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DirectoryNotFound,
                    "directory is absent from the current workspace",
                )
            })?;
        (entry.entry_id.clone(), entry.path().to_owned())
    };
    let mut scopes = entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && (key.is_empty() || namespace::is_descendant(entry.lookup_key(), &key))
        })
        .map(|entry| (entry.entry_id.clone(), entry.path().to_owned()))
        .collect::<Vec<_>>();
    scopes.sort_by_key(|(_, path)| std::cmp::Reverse(path.split('/').count()));
    scopes.push((directory_id, path));
    let mut selected = None;
    for (directory_id, path) in scopes {
        let source = DirectorySnapshotSource {
            directory_id,
            entries: current_scope_entries(&entries, &name_policy::path_key(&path)).to_vec(),
            path,
            current: Arc::clone(&current),
        };
        selected = Some(snapshot_source_with_ids(state, repository, source, ids)?);
    }
    Ok(selected.expect("the requested scope is always captured last"))
}

/// Namespace mutations change physical ownership; selecting an alias does not.
pub(crate) fn set_namespace_location(
    state: &mut State,
    file: &Entry,
    source: &str,
    destination: &str,
    kind: DirectoryBatchKind,
    is_directory: bool,
) {
    let old = state
        .file_locations
        .get(&file.file_id)
        .cloned()
        .unwrap_or_else(|| file.path().to_owned());
    let path = namespace_location(old, file, source, destination, kind, is_directory);
    state.file_names.insert(
        file.file_id.clone(),
        path.rsplit('/').next().unwrap_or("").into(),
    );
    state.file_locations.insert(file.file_id.clone(), path);
}

fn namespace_location(
    old: String,
    file: &Entry,
    source: &str,
    destination: &str,
    kind: DirectoryBatchKind,
    is_directory: bool,
) -> String {
    let same_parent = name_policy::path_key(source.rsplit_once('/').map(|v| v.0).unwrap_or(""))
        == name_policy::path_key(destination.rsplit_once('/').map(|v| v.0).unwrap_or(""));
    if kind == DirectoryBatchKind::Rename && same_parent {
        if is_directory {
            let key = name_policy::path_key(source);
            let owner = name_policy::path_key(&old);
            if namespace::is_descendant(&owner, &key) {
                let depth = source.split('/').count();
                format!(
                    "{destination}/{}",
                    old.split('/').skip(depth).collect::<Vec<_>>().join("/")
                )
            } else {
                old
            }
        } else {
            let name = destination.rsplit('/').next().unwrap_or(destination);
            old.rsplit_once('/')
                .map(|(parent, _)| format!("{parent}/{name}"))
                .unwrap_or_else(|| name.into())
        }
    } else {
        let depth = source.split('/').count();
        let suffix = file
            .path()
            .split('/')
            .skip(depth)
            .collect::<Vec<_>>()
            .join("/");
        if suffix.is_empty() {
            destination.into()
        } else {
            format!("{destination}/{suffix}")
        }
    }
}

/// Prepare real directory identities before changing their display path, so
/// rename produces a child subtree even for empty directories.
pub(crate) fn prepare_renamed_directories(
    state: &mut State,
    repository: &str,
    before: &[Entry],
    changes: &[NamespaceOperation],
) -> Result<Vec<String>> {
    let mut renamed = Vec::new();
    let mut old_paths = Vec::new();
    for dir in before
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
    {
        let mut path = std::borrow::Cow::Borrowed(dir.location());
        for change in changes {
            let NamespaceOperation::Rename {
                source,
                destination,
            } = change
            else {
                continue;
            };
            if path.key() == source.key() {
                path = std::borrow::Cow::Owned(destination.clone());
            } else if namespace::is_descendant(path.key(), source.key()) {
                path = std::borrow::Cow::Owned(
                    path.rebase(source.as_str().split('/').count(), destination)?,
                );
            }
        }
        if path.as_str() != dir.path() {
            old_paths.push(dir.path().to_owned());
            renamed.push(format!("{path}/"));
        }
    }
    old_paths.sort_by_key(|p| std::cmp::Reverse(p.split('/').count()));
    old_paths.dedup();
    for path in old_paths {
        let (id, _, _) = scope(state, repository, &path)?;
        if !state.directory_selections.contains_key(&id) {
            snapshot(state, repository, &path)?;
        }
    }
    Ok(renamed)
}

/// Ordinary namespace renames freeze only the admitted source entries. Derive
/// surviving moved directories from the final draft, so Rename/DeleteTree and
/// chained moves cannot snapshot deleted paths or revive hidden descendants.
pub(crate) fn prepare_current_renamed_directories(
    state: &mut State,
    repository: &str,
    before: &[Entry],
    after: &[Entry],
    changes: &[NamespaceOperation],
) -> Result<Vec<String>> {
    if !changes
        .iter()
        .any(|change| change.kind() == NamespaceOperationKind::Rename)
    {
        return Ok(Vec::new());
    }
    let originals: BTreeMap<_, _> = before
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
        .map(|entry| (entry.entry_id.as_str(), entry))
        .collect();
    let mut renamed = Vec::new();
    let mut previous = Vec::new();
    for directory in after
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
    {
        match originals.get(directory.entry_id.as_str()) {
            Some(original) if original.path() == directory.path() => {}
            original => {
                renamed.push(format!("{}/", directory.path()));
                if let Some(original) = original
                    && !state.directory_selections.contains_key(&original.entry_id)
                {
                    previous.push(*original);
                }
            }
        }
    }
    previous.sort_by_key(|entry| std::cmp::Reverse(entry.path().split('/').count()));
    let repo = required(
        state.repositories.get(repository),
        "directory repository missing",
    )?;
    let head = required(
        state.branches.get(&repo.main_branch_id),
        "repository main branch missing",
    )?;
    let current = state
        .commits
        .get_shared(&head.head_commit_id)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "repository head missing",
            )
        })?;
    for original in previous {
        let prefix = format!("{}/", original.lookup_key());
        let start = before.partition_point(|entry| entry.lookup_key() < prefix.as_str());
        snapshot_source(
            state,
            repository,
            DirectorySnapshotSource {
                directory_id: original.entry_id.clone(),
                path: original.path().into(),
                current: Arc::clone(&current),
                entries: before[start..]
                    .iter()
                    .take_while(|entry| entry.lookup_key().starts_with(&prefix))
                    .cloned()
                    .collect(),
            },
        )?;
    }
    Ok(renamed)
}

/// Batch mappings name entries in their original frozen scopes, not the output
/// of earlier mappings. Keep that contract separate from ordered workspaces.
pub(crate) fn prepare_independent_renamed_directories(
    state: &mut State,
    repository: &str,
    before: &[Entry],
    changes: &[NamespaceOperation],
) -> Result<Vec<String>> {
    let mut renamed = Vec::new();
    for change in changes {
        renamed.extend(prepare_renamed_directories(
            state,
            repository,
            before,
            std::slice::from_ref(change),
        )?);
    }
    Ok(renamed)
}

pub(crate) fn namespace_locations(
    state: &mut State,
    repository: &str,
    before: &[Entry],
    after: &[Entry],
    changes: &[NamespaceOperation],
    principal: &str,
    operation: OperationId,
) -> Result<()> {
    // Workspace entries may not be sorted yet. Keep the first path match,
    // just like `find`, without rescanning all siblings for every file.
    let mut after_by_path = BTreeMap::new();
    for entry in after {
        after_by_path.entry(entry.lookup_key()).or_insert(entry);
    }
    let original_contexts = before
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .filter_map(|e| {
            context_at_key(state, repository, e.lookup_key())
                .map(|c| (context_key_from_key(repository, e.lookup_key()), c))
        })
        .collect::<BTreeMap<_, _>>();
    let mut physical_outcomes = BTreeMap::<String, String>::new();
    let mut contexts = BTreeMap::new();
    for old in before.iter().filter(|e| e.kind == NamespaceEntryKind::File) {
        // Replay each entry's path independently. Looking only at the first
        // rename loses chained moves; matching by file ID alone confuses aliases.
        let mut moved = old.clone();
        let mut renamed = false;
        let mut deleted = false;
        let mut physical_path = state
            .file_locations
            .get(&old.file_id)
            .cloned()
            .unwrap_or_else(|| old.path().to_owned());
        let original_physical_path = physical_path.clone();
        for change in changes {
            let source = change.key();
            if (change.kind() == NamespaceOperationKind::Delete && moved.lookup_key() == source)
                || (change.kind() == NamespaceOperationKind::DeleteTree
                    && (moved.lookup_key() == source
                        || namespace::is_descendant(moved.lookup_key(), source)))
            {
                deleted = true;
                break;
            }
            let NamespaceOperation::Rename { destination, .. } = change else {
                continue;
            };
            let path = if moved.lookup_key() == source {
                destination.clone()
            } else if namespace::is_descendant(moved.lookup_key(), source) {
                moved
                    .location()
                    .rebase(change.path().split('/').count(), destination)?
            } else {
                continue;
            };
            if path.as_str() == moved.path() {
                continue;
            }
            physical_path = namespace_location(
                physical_path,
                &moved,
                change.path(),
                destination,
                DirectoryBatchKind::Rename,
                moved.lookup_key() != source,
            );
            moved.relocate(path);
            renamed = true;
        }
        if !deleted
            && let Some(current) = after_by_path.get(moved.lookup_key()).filter(|e| {
                e.kind == NamespaceEntryKind::File
                    && e.entry_id == old.entry_id
                    && e.file_id == old.file_id
            })
        {
            let context = original_contexts
                .get(&context_key_from_key(repository, old.lookup_key()))
                .cloned();
            let context = if current.path() != old.path() {
                Some(file_paths::rename_node(
                    state, repository, old, current, context, principal, operation,
                )?)
            } else {
                state
                    .file_contexts
                    .get(&context_key_from_key(repository, old.lookup_key()))
                    .cloned()
            };
            if let Some(context) = context {
                if current.path() != old.path() {
                    file::relocate_namespace_file_branch(
                        state,
                        &context,
                        &after_by_path,
                        current,
                        operation,
                    )?;
                }
                contexts.insert(
                    context_key_from_key(repository, current.lookup_key()),
                    context,
                );
            }
            // Renaming a non-owning alias directory does not propose a
            // competing physical location. Compare net outcomes so a
            // temporary move followed by its reversal is also a no-op.
            if renamed && physical_path != original_physical_path {
                let outcome = physical_path;
                if let Some(previous) =
                    physical_outcomes.insert(old.file_id.clone(), outcome.clone())
                    && name_policy::path_key(&previous) != name_policy::path_key(&outcome)
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::BatchMappingConflict,
                        "namespace changes give one physical file incompatible names or locations",
                    ));
                }
            }
        }
    }
    for (file, path) in physical_outcomes {
        state
            .file_names
            .insert(file.clone(), path.rsplit('/').next().unwrap_or("").into());
        state.file_locations.insert(file, path);
    }
    // Remove old keys together before installing new keys, so a path swap
    // cannot erase the context already installed for the other file.
    for old in before.iter().filter(|e| e.kind == NamespaceEntryKind::File) {
        state
            .file_contexts
            .remove(&context_key_from_key(repository, old.lookup_key()));
    }
    state.file_contexts.extend(contexts);
    let live = after
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .map(|e| e.file_id.as_str())
        .collect::<BTreeSet<_>>();
    for old in before
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File && !live.contains(e.file_id.as_str()))
    {
        state.file_selections.remove(&old.file_id);
        state.file_write_origins.remove(&old.file_id);
    }
    Ok(())
}

pub(crate) fn unwritten_source_at<'a>(
    state: &'a State,
    branch: &str,
    key: &str,
) -> Option<&'a str> {
    branch_context_at_key(state, branch, key)?
        .inherited_from
        .as_ref()?;
    namespace::find_unordered_key(
        &state
            .commits
            .get(&state.branches.get(branch)?.head_commit_id)?
            .entries,
        key,
    )
    .map(|entry| entry.version_id.as_str())
}

// Persisted directory labels are admitted as NFC portable branch names. A nonzero
// ASCII suffix follows a UTF-8 prefix and keeps the result within 128 bytes.
fn fork_name(origin_name: &str, number: u64) -> (String, String) {
    let suffix = if number == 0 {
        String::new()
    } else {
        format!("-{number}")
    };
    let limit = 128 - suffix.len();
    let mut end = origin_name.len().min(limit);
    while !origin_name.is_char_boundary(end) {
        end -= 1;
    }
    let name = format!("{}{suffix}", &origin_name[..end]);
    let key = name_policy::lookup_key(&name);
    (name, key)
}

/// Fork inherited file intent only at the first actual save. An explicit file
/// selection removes the intent and continues the selected public branch.
pub(crate) fn prepare_file_write(
    engine: &crate::engine::PlanningContext<'_>,
    state: &mut State,
    branch: &str,
    file_id: &str,
    key: &str,
    principal: &str,
    operation: OperationId,
) -> Result<()> {
    let Some(repository) =
        repository::main_for_branch(state, branch).map(|r| r.repository_id.clone())
    else {
        return Ok(());
    };
    let Some(origin) = context_at_key(state, &repository, key).and_then(|c| c.inherited_from)
    else {
        return Ok(());
    };
    let head = required(state.branches.get(branch), "repository main branch missing")?;
    let source = required(
        state.commits.get(&head.head_commit_id),
        "repository head commit missing",
    )?;
    let entries = match engine.current_namespaces {
        Some(view) => view.branch_entries(branch, &head.head_commit_id)?,
        None => source.entries.as_slice(),
    };
    let original = entries
        .iter()
        .find(|entry| entry.lookup_key() == key && entry.kind == NamespaceEntryKind::File)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNotFound,
                "inherited file missing from the current namespace",
            )
        })?;
    let origin_name = state
        .directory_branches
        .get(&origin)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "inherited directory origin missing",
            )
        })?
        .name
        .clone();
    let unique = |key: &str| {
        !state.branches.values().any(|b| {
            b.file_id.as_deref() == Some(file_id) && name_policy::lookup_key(&b.name) == key
        })
    };
    let mut number = 0u64;
    let (name, name_key) = loop {
        let (candidate, key) = fork_name(&origin_name, number);
        if key != "main" && unique(&key) {
            break (candidate, key);
        }
        number = number.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "file branch name exhausted",
            )
        })?;
    };
    let result = file::create_file_branch_change(
        engine,
        state,
        file::FileBranchCreation {
            file_id,
            source_version: &original.version_id,
            name: &name,
            name_key: &name_key,
        },
        principal,
        operation,
    )?;
    let id = result["branch_id"].as_str().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "file fork receipt missing",
        )
    })?;
    let inherited_node = context_at_key(state, &repository, key).and_then(|c| c.node_id);
    state
        .branches
        .get_mut(id)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "created file branch missing",
            )
        })?
        .file_node_id = inherited_node.clone();
    state.file_selections.insert(file_id.into(), id.into());
    state.file_write_origins.remove(file_id);
    state.file_contexts.insert(
        context_key_from_key(&repository, key),
        FileContext {
            node_id: inherited_node.clone(),
            selection_generation: 0,
            branch_id: id.into(),
            inherited_from: None,
        },
    );
    Ok(())
}

/// Refresh every selected ancestor affected by the current registry, even when
/// this workspace writes an unrelated retained file. Otherwise a new clean root
/// could retain an immutable child reference containing a hidden occurrence.
pub(crate) fn record_projected_directory_edits(
    state: &mut State,
    repository: &str,
    changed_paths: &[String],
    exclude: Option<&str>,
    view: Option<&crate::engine::CurrentNamespaces>,
) -> Result<Vec<String>> {
    let Some(view) = view else {
        return record_directory_edits(state, repository, changed_paths, exclude);
    };
    let repo = required(
        state.repositories.get(repository),
        "directory repository missing",
    )?;
    let paths = changed_paths
        .iter()
        .cloned()
        .chain(view.removed_paths(&repo.main_branch_id).map(str::to_owned))
        .collect::<Vec<_>>();
    record_directory_edits(state, repository, &paths, exclude)
}

pub(crate) fn record_directory_edits(
    state: &mut State,
    repository: &str,
    changed_paths: &[String],
    exclude: Option<&str>,
) -> Result<Vec<String>> {
    let repo = required(
        state.repositories.get(repository),
        "directory repository missing",
    )?;
    if !repo.file_manager_root
        && !state
            .directory_branches
            .values()
            .any(|b| b.repository_id == repository)
        && !state.file_selections.values().any(|id| {
            state
                .branches
                .get(id)
                .is_some_and(|b| b.history_tree_id == repo.history_tree_id)
        })
    {
        return Ok(Vec::new());
    }
    let mut paths = BTreeSet::new();
    for path in changed_paths {
        paths.insert(String::new());
        let parts = path.split('/').collect::<Vec<_>>();
        for end in 1..parts.len() {
            paths.insert(parts[..end].join("/"));
        }
    }
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    // The repository root has no components. Publish it after top-level
    // directories too, so its frozen child references see their new trees.
    paths.sort_by_key(|p| {
        std::cmp::Reverse(if p.is_empty() {
            0
        } else {
            p.split('/').count()
        })
    });
    let mut result = Vec::new();
    for path in paths {
        if exclude.is_some_and(|key| name_policy::path_key(&path) == key) {
            continue;
        }
        match scope(state, repository, &path) {
            Err(e) if e.code() == storage_contract::ErrorCode::DirectoryNotFound => continue,
            Err(e) => return Err(e),
            Ok(_) => result.push(snapshot(state, repository, &path)?),
        }
    }
    Ok(result)
}

impl Engine {
    pub fn file_context(&self, repository: &str, path: &str) -> Option<FileContext> {
        let key = name_policy::path_key(path);
        let repo = self.state.repositories.get(repository)?;
        match self.namespace_read_view() {
            crate::engine::NamespaceReadView::AwaitingPublication => return None,
            crate::engine::NamespaceReadView::Published(view) => {
                let branch = self.state.branches.get(&repo.main_branch_id)?;
                if !view.contains_entry(&repo.main_branch_id, &branch.head_commit_id, &key, None) {
                    return None;
                }
            }
            crate::engine::NamespaceReadView::Initial => {}
        }
        context_at_key(&self.state, repository, &key)
    }

    /// Resolve an entry already borrowed from this repository's current main
    /// commit. The caller must not pass an entry from another snapshot. Unlike
    /// `file_context`, this avoids looking the same entry up by path again.
    pub fn file_context_for_current_entry<'a>(
        &'a self,
        repository: &str,
        entry: &Entry,
    ) -> Option<std::borrow::Cow<'a, FileContext>> {
        match self.namespace_read_view() {
            crate::engine::NamespaceReadView::AwaitingPublication => return None,
            crate::engine::NamespaceReadView::Published(view) => {
                let repo = self.state.repositories.get(repository)?;
                let branch = self.state.branches.get(&repo.main_branch_id)?;
                if !view.contains_entry(
                    &repo.main_branch_id,
                    &branch.head_commit_id,
                    entry.lookup_key(),
                    Some(entry),
                ) {
                    return None;
                }
            }
            crate::engine::NamespaceReadView::Initial => {}
        }
        if let Some(context) = self
            .state
            .file_contexts
            .get(&context_key_from_key(repository, entry.lookup_key()))
        {
            return Some(std::borrow::Cow::Borrowed(context));
        }
        let repo = self.state.repositories.get(repository)?;
        (entry.kind == NamespaceEntryKind::File)
            .then(|| std::borrow::Cow::Owned(context_for_entry(&self.state, repo, entry)))
    }
    /// Explicitly initialize a directory's first composition identity.
    /// A new namespace need not have a named view; registration copies no data.
    pub fn initialize_directory_scope(
        &mut self,
        repository: &str,
        path: Option<&name_policy::LookupPath>,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "directory.scope.initialize",
            principal,
            operation,
            json!({"repository_id":repository,"path":path.map_or("",name_policy::LookupPath::as_str)}),
            |engine, state, _| {
                let view = DirectorySnapshotSource::current_view(
                    engine,
                    repository,
                    Cow::Borrowed(path.map_or("", name_policy::LookupPath::key)),
                )?;
                let id = view.directory_id.to_owned();
                if engine.current_namespaces.is_some() {
                    let source = DirectorySnapshotSource::capture(&engine.state, view);
                    snapshot_current_source(state, repository, source)?;
                } else if !state.directory_selections.contains_key(&id) {
                    let source = DirectorySnapshotSource::capture(&engine.state, view);
                    snapshot_source(state, repository, source)?;
                }
                let (branch, tree) = selected_tree(state, &id)?;
                Ok(json!({"directory_branch_id":branch,"head_tree_id":tree.tree_id}))
            },
        )
    }
    pub fn create_directory_branches(
        &mut self,
        repository: &str,
        path: Option<&name_policy::LookupPath>,
        name: &storage_contract::BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let input = json!({"repository_id":repository,"path":path.map_or("",name_policy::LookupPath::as_str),"name":name.as_str()});
        self.mutate(
            "directory.git.branch-create",
            principal,
            operation,
            input.clone(),
            |engine, state, _| {
                directory_git::apply_command(
                    state,
                    engine.admitted_current_namespaces()?,
                    "directory.git.branch-create",
                    &input,
                    principal,
                    operation,
                )
            },
        )
    }
    pub fn select_directory_branches(
        &mut self,
        repository: &str,
        path: Option<&name_policy::LookupPath>,
        name: &storage_contract::BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let input = json!({"repository_id":repository,"path":path.map_or("",name_policy::LookupPath::as_str),"name":name.as_str()});
        self.directory_git_checkout(input, principal, operation)
    }
    pub fn directory_branches(
        &self,
        repository: &str,
        path: &str,
    ) -> Result<Vec<storage_contract::DirectoryBranchSummary>> {
        if !path.is_empty() {
            name_policy::validate_lookup_path(path)?;
        }
        let view = DirectorySnapshotSource::current_read_view(
            self,
            repository,
            Cow::Owned(name_policy::path_key(path)),
        )?;
        let mut summaries = storage_contract::directory_branch_summaries(
            &self.state,
            storage_contract::DirectoryBranchScope {
                repository_id: repository,
                directory_id: view.directory_id,
                path,
            },
        );
        summaries.retain(|summary| {
            self.state
                .directory_git_repositories
                .get(view.directory_id)
                .is_some_and(|git| git.branches.contains_key(&summary.directory_branch_id))
        });
        if let Some(current) = self.admitted_current_namespaces()? {
            for summary in &mut summaries {
                summary.file_count = current
                    .directory_entries(&summary.directory_branch_id, &summary.head_tree_id)?
                    .iter()
                    .filter(|entry| entry.kind == NamespaceEntryKind::File)
                    .map(|entry| entry.file_id.as_str())
                    .collect::<BTreeSet<_>>()
                    .len();
            }
        }
        Ok(summaries)
    }
    pub fn select_default_directory_branch(
        &mut self,
        repository: &str,
        branch: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let input =
            json!({"repository_id":repository,"directory_branch_id":branch,"root_only":true});
        self.directory_git_checkout(input, principal, operation)
    }
    pub fn rename_directory_branch(
        &mut self,
        repository: &str,
        branch: &str,
        name: &storage_contract::BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "directory.branch.rename",
            principal,
            operation,
            json!({"repository_id":repository,"directory_branch_id":branch,"name":name.as_str()}),
            |engine, state, _| {
                let repo = state.repositories.get(repository).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::RepositoryNotFound,
                        "repository missing",
                    )
                })?;

                let current = state
                    .directory_branches
                    .get(branch)
                    .filter(|b| b.repository_id == repository)
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::DirectoryBranchNotFound,
                            "directory branch missing",
                        )
                    })?;
                if !state
                    .directory_git_repositories
                    .get(&current.directory_id)
                    .is_some_and(|git| git.branches.contains_key(branch))
                {
                    return Err(Error::new(
                        ErrorCode::DirectoryScopeRequired,
                        "initialize this directory repository before changing its branches",
                    ));
                }
                if let Some(view) = engine.admitted_current_namespaces()? {
                    let owner = engine.namespace_tree(&repo.main_branch_id)?;
                    view.directory_entries(branch, &current.head_tree_id)?;
                    let visible = if current.directory_id == repo.repository_id {
                        // Disabled roots still exist. A detached root is only a
                        // retained-content scope, never a newly promoted primary.
                        !owner.entries().is_empty()
                            || engine
                                .registered_disks()
                                .any(|disk| disk.slot_id == repo.primary_slot_id)
                    } else {
                        owner.entries().iter().any(|entry| {
                            entry.kind == NamespaceEntryKind::Directory
                                && entry.entry_id == current.directory_id
                        })
                    };
                    if !visible {
                        return Err(Error::new(
                            ErrorCode::DirectoryBranchNotFound,
                            "directory identity is absent from the current namespace",
                        ));
                    }
                }
                if state.directory_branches.values().any(|b| {
                    b.directory_id == current.directory_id
                        && b.directory_branch_id != branch
                        && name_policy::lookup_key(&b.name) == name.key()
                }) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::NameCollision,
                        "directory branch name exists",
                    ));
                }
                let target = state.directory_branches.get_mut(branch).expect(
                    "invariant: the directory branch was resolved in this unchanged candidate",
                );
                target.name = name.as_str().into();
                Ok(json!({"directory_branch_id":branch,"name":name.as_str(),
                    "path":target.path,"head_tree_id":target.head_tree_id}))
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/directory_corruption_tests.rs"]
mod corruption_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/directory_fork_names.rs"]
mod fork_names;
