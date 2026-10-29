//! Frozen directory compositions over public file branches and immutable nodes.
use super::*;
use json_codec::{Value, json};
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

fn frozen_context(
    contexts: &BTreeMap<String, FileContext>,
    branches: &BTreeMap<String, String>,
    entry: &Entry,
) -> Result<FileContext> {
    // Exact path context and public branch ownership are independently required.
    // Never manufacture a missing frozen context from another identity map.
    required(
        branches.get(&entry.file_id),
        "snapshot file branch reference missing",
    )?;
    required(
        contexts.get(entry.lookup_key()),
        "snapshot file path context missing",
    )
    .cloned()
}

pub(crate) fn context_key_from_key(repository: &str, key: &str) -> String {
    format!("{repository}/{key}")
}
pub(crate) fn context_at_key(
    state: &State,
    repository: &str,
    key: &str,
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<Option<FileContext>> {
    if let Some(context) = state
        .file_contexts
        .get(&context_key_from_key(repository, key))
    {
        return Ok(Some(context.clone()));
    }
    let Some(repo) = state.repositories.get(repository) else {
        return Ok(None);
    };
    let Some(branch) = state.branches.get(&repo.main_branch_id) else {
        return Ok(None);
    };
    let Some(commit) = state.commits.get(&branch.head_commit_id) else {
        return Ok(None);
    };
    let entry = crate::namespace::NamespaceView::new(&commit.namespace, source)
        .resolve_key(key)?
        .filter(|e| e.kind == NamespaceEntryKind::File);
    Ok(entry.map(|entry| context_for_entry(state, repo, &entry)))
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
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<Option<FileContext>> {
    let Some(branch) = state.branches.get(namespace) else {
        return Ok(None);
    };
    if let Some(repo) = repository::main_for_branch(state, namespace) {
        context_at_key(state, &repo.repository_id, key, source)
    } else {
        Ok(Some(FileContext {
            node_id: branch.file_node_id.clone(),
            selection_generation: 0,
            branch_id: branch.branch_id.clone(),
            inherited_from: None,
        }))
    }
}

struct DirectoryScopeView<'a> {
    directory_id: String,
    path: String,
    key: Cow<'a, str>,
    commit: &'a Commit,
}
fn scope_view<'a>(
    state: &'a State,
    repository: &str,
    path: &str,
    source: &dyn crate::namespace::NamespaceSource,
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
    scope_view_for_key(state, repo, Cow::Owned(name_policy::path_key(path)), source)
}
fn scope_view_for_key<'a>(
    state: &'a State,
    repo: &'a Repository,
    key: Cow<'a, str>,
    source: &dyn crate::namespace::NamespaceSource,
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
        (repo.repository_id.clone(), String::new())
    } else {
        // One point read locates the directory; no tree scan.
        let entry = crate::namespace::NamespaceView::new(&commit.namespace, source)
            .resolve_key(key.as_ref())?
            .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::DirectoryNotFound,
                    "directory missing",
                )
            })?;
        let path = entry.path().to_owned();
        (entry.entry_id, path)
    };
    Ok(DirectoryScopeView {
        directory_id,
        path,
        key,
        commit,
    })
}
/// The scope's descendant entries (excluding the scope directory itself) in
/// canonical order; the complete tree for the root scope. O(subtree).
fn scope_entries(
    view: &DirectoryScopeView<'_>,
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<Vec<Entry>> {
    let ns = crate::namespace::NamespaceView::new(&view.commit.namespace, source);
    if view.key.is_empty() {
        return ns.collect_entries();
    }
    let mut out = Vec::new();
    ns.walk_subtree(view.key.as_ref(), |_, entry| {
        if entry.lookup_key() != view.key.as_ref() {
            out.push(entry.clone());
        }
        Ok(true)
    })?;
    out.sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
    Ok(out)
}
struct DirectorySnapshotSource {
    directory_id: String,
    path: String,
    current: Arc<Commit>,
    entries: Vec<Entry>,
}
impl DirectorySnapshotSource {
    fn capture(
        state: &State,
        view: DirectoryScopeView<'_>,
        source: &dyn crate::namespace::NamespaceSource,
    ) -> Result<Self> {
        let entries = scope_entries(&view, source)?;
        Ok(Self {
            directory_id: view.directory_id,
            path: view.path,
            current: state
                .commits
                .get_shared(&view.commit.commit_id)
                .expect("invariant: scope view borrows an existing commit"),
            entries,
        })
    }
}

pub(crate) fn scope(
    state: &State,
    repository: &str,
    path: &str,
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<(String, Vec<Entry>, String)> {
    let view = scope_view(state, repository, path, source)?;
    let entries = scope_entries(&view, source)?;
    Ok((view.directory_id, entries, view.commit.commit_id.clone()))
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

fn snapshot(
    state: &mut State,
    repository: &str,
    path: &str,
    base: &dyn crate::namespace::NamespaceSource,
    fresh: &mut crate::namespace::NamespaceObjects,
) -> Result<String> {
    let source =
        DirectorySnapshotSource::capture(state, scope_view(state, repository, path, base)?, base)?;
    snapshot_source(state, repository, source, base, fresh)
}
fn snapshot_source(
    state: &mut State,
    repository: &str,
    source: DirectorySnapshotSource,
    base: &dyn crate::namespace::NamespaceSource,
    fresh: &mut crate::namespace::NamespaceObjects,
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
            file::ensure_shared_file_branch(state, &current, e, base, fresh)?,
        );
    }
    let draft = crate::namespace::DraftSource {
        fresh: &*fresh,
        base,
    };
    let file_contexts = entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .map(|e| {
            let branch_id =
                required(files.get(&e.file_id), "snapshot file branch missing")?.clone();
            let context =
                context_at_key(state, repository, e.lookup_key(), &draft)?.unwrap_or(FileContext {
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
    let id = previous.unwrap_or_else(|| new_id("dbr"));
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
    let tree_id = new_id("dtree");
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

pub(crate) fn snapshot_pending(
    state: &State,
    entries: &[Entry],
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<BTreeMap<String, String>> {
    let mut pending = BTreeMap::new();
    for e in entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
    {
        let context = match state.repositories.values().find(|r| {
            state
                .versions
                .get(&e.version_id)
                .is_some_and(|v| v.history_tree_id == r.history_tree_id)
        }) {
            Some(r) => context_at_key(state, &r.repository_id, e.lookup_key(), source)?,
            None => None,
        };
        let Some(c) = context else {
            continue;
        };
        if c.inherited_from.is_some()
            || state
                .branches
                .get(&c.branch_id)
                .is_some_and(|b| b.pending_file_versions.contains_key(&e.file_id))
        {
            pending.insert(e.file_id.clone(), e.version_id.clone());
        }
    }
    Ok(pending)
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
/// The pre-edit key of `key`, replayed backward through the earlier
/// operations' renames. A superset guard: candidates found through it are
/// still replayed forward individually, so imprecision never changes results.
fn preimage_key(mut key: String, earlier: &[NamespaceOperation]) -> String {
    for change in earlier.iter().rev() {
        let NamespaceOperation::Rename {
            source,
            destination,
        } = change
        else {
            continue;
        };
        let (from, to) = (destination.key(), source.key());
        if key == from {
            key = to.to_owned();
        } else if namespace::is_descendant(&key, from) {
            key = format!("{to}{}", &key[from.len()..]);
        }
    }
    key
}

/// The directory entries that any operation's source subtree can touch:
/// the rename/delete sources (and their pre-edit images for chained renames)
/// plus their descendants, out of the pre-edit namespace. Untouched subtrees
/// are never read.
fn candidate_directories(
    before: &storage_contract::CommitNamespace,
    changes: &[NamespaceOperation],
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<Vec<Entry>> {
    let view = crate::namespace::NamespaceView::new(before, source);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, change) in changes.iter().enumerate() {
        if !matches!(change, NamespaceOperation::Rename { .. }) {
            continue;
        }
        let key = preimage_key(change.key().to_owned(), &changes[..index]);
        for entry in view.subtree_directories(&key)? {
            if seen.insert(entry.entry_id.clone()) {
                out.push(entry);
            }
        }
    }
    Ok(out)
}

pub(crate) fn prepare_renamed_directories(
    state: &mut State,
    repository: &str,
    before: &storage_contract::CommitNamespace,
    changes: &[NamespaceOperation],
    source: &dyn crate::namespace::NamespaceSource,
    fresh: &mut crate::namespace::NamespaceObjects,
) -> Result<Vec<String>> {
    if !changes
        .iter()
        .any(|change| matches!(change, NamespaceOperation::Rename { .. }))
    {
        return Ok(Vec::new());
    }
    let mut renamed = Vec::new();
    let mut old_paths = Vec::new();
    for dir in candidate_directories(before, changes, source)? {
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
        let (id, _, _) = scope(state, repository, &path, source)?;
        if !state.directory_selections.contains_key(&id) {
            snapshot(state, repository, &path, source, fresh)?;
        }
    }
    Ok(renamed)
}

/// Batch mappings name entries in their original frozen scopes, not the output
/// of earlier mappings. Keep that contract separate from ordered workspaces.
pub(crate) fn prepare_independent_renamed_directories(
    state: &mut State,
    repository: &str,
    before: &storage_contract::CommitNamespace,
    changes: &[NamespaceOperation],
    source: &dyn crate::namespace::NamespaceSource,
    fresh: &mut crate::namespace::NamespaceObjects,
) -> Result<Vec<String>> {
    let mut renamed = Vec::new();
    for change in changes {
        renamed.extend(prepare_renamed_directories(
            state,
            repository,
            before,
            std::slice::from_ref(change),
            source,
            fresh,
        )?);
    }
    Ok(renamed)
}

pub(crate) fn namespace_locations(
    state: &mut State,
    repository: &str,
    before: &storage_contract::CommitNamespace,
    after: &storage_contract::CommitNamespace,
    changes: &[NamespaceOperation],
    provenance: OperationContext<'_>,
    namespace: NamespaceMutation<'_>,
) -> Result<()> {
    let NamespaceMutation { base, fresh } = namespace;
    // No operations means no path moved: a proven no-op, skipped outright.
    if changes.is_empty() {
        return Ok(());
    }
    // Only files under an operation's source subtree (or its pre-edit image
    // for chained renames) can be touched; everything else keeps its context.
    let affected: Vec<Entry> = {
        let draft = crate::namespace::DraftSource {
            fresh: &*fresh,
            base,
        };
        let view = crate::namespace::NamespaceView::new(before, &draft);
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for (index, change) in changes.iter().enumerate() {
            if matches!(change, NamespaceOperation::Mkdir { .. }) {
                continue;
            }
            let key = preimage_key(change.key().to_owned(), &changes[..index]);
            let Some(root) = view.resolve_key(&key)? else {
                continue;
            };
            // Dedupe by path key, never by identity: logical aliases share
            // one stable entry identity at several paths, and each path's
            // context migrates independently.
            match root.kind {
                NamespaceEntryKind::File => {
                    if seen.insert(root.lookup_key().to_owned()) {
                        out.push(root);
                    }
                }
                NamespaceEntryKind::Directory => {
                    view.walk_subtree(&key, |_, entry| {
                        if entry.kind == NamespaceEntryKind::File
                            && seen.insert(entry.lookup_key().to_owned())
                        {
                            out.push(entry.clone());
                        }
                        Ok(true)
                    })?;
                }
            }
        }
        out
    };
    let original_contexts = {
        let draft = crate::namespace::DraftSource {
            fresh: &*fresh,
            base,
        };
        let mut contexts = BTreeMap::new();
        for e in affected
            .iter()
            .filter(|e| e.kind == NamespaceEntryKind::File)
        {
            if let Some(c) = context_at_key(state, repository, e.lookup_key(), &draft)? {
                contexts.insert(context_key_from_key(repository, e.lookup_key()), c);
            }
        }
        contexts
    };
    let mut physical_outcomes = BTreeMap::<String, String>::new();
    let mut contexts = BTreeMap::new();
    let mut removed_keys = Vec::new();
    let mut maybe_dead = BTreeMap::<String, String>::new();
    for old in affected
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
    {
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
        removed_keys.push(old.lookup_key().to_owned());
        if deleted {
            maybe_dead.insert(old.file_id.clone(), old.lookup_key().to_owned());
        }
        let current = if deleted {
            None
        } else {
            let draft = crate::namespace::DraftSource {
                fresh: &*fresh,
                base,
            };
            crate::namespace::NamespaceView::new(after, &draft)
                .resolve_key(moved.lookup_key())?
                .filter(|e| {
                    e.kind == NamespaceEntryKind::File
                        && e.entry_id == old.entry_id
                        && e.file_id == old.file_id
                })
        };
        if let Some(current) = current.as_ref() {
            let context = original_contexts
                .get(&context_key_from_key(repository, old.lookup_key()))
                .cloned();
            let context = if current.path() != old.path() {
                Some(file_paths::rename_node(
                    state,
                    repository,
                    old,
                    current,
                    context,
                    provenance,
                    NamespaceMutation { base, fresh },
                )?)
            } else {
                state
                    .file_contexts
                    .get(&context_key_from_key(repository, old.lookup_key()))
                    .cloned()
            };
            if let Some(context) = context {
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
    // Remove the affected old keys together before installing new keys, so a
    // path swap cannot erase the context already installed for the other
    // file. Untouched files keep their contexts without any churn.
    for key in removed_keys {
        state
            .file_contexts
            .remove(&context_key_from_key(repository, &key));
    }
    state.file_contexts.extend(contexts);
    // A deleted entry's file is gone unless another entry still carries it:
    // one walk of the post-edit tree, stopping once every candidate is seen.
    if !maybe_dead.is_empty() {
        let draft = crate::namespace::DraftSource {
            fresh: &*fresh,
            base,
        };
        crate::namespace::NamespaceView::new(after, &draft).walk(|_, entry| {
            if let storage_contract::NamespacePageEntry::File { file_id, .. } = entry {
                maybe_dead.remove(file_id);
            }
            Ok(!maybe_dead.is_empty())
        })?;
        for file in maybe_dead.keys() {
            state.file_selections.remove(file);
            state.file_write_origins.remove(file);
        }
    }
    Ok(())
}

pub(crate) fn unwritten_source_at(
    state: &State,
    branch: &str,
    key: &str,
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<Option<String>> {
    if branch_context_at_key(state, branch, key, source)?
        .and_then(|context| context.inherited_from)
        .is_none()
    {
        return Ok(None);
    }
    let Some(branch) = state.branches.get(branch) else {
        return Ok(None);
    };
    let Some(commit) = state.commits.get(&branch.head_commit_id) else {
        return Ok(None);
    };
    Ok(
        crate::namespace::NamespaceView::new(&commit.namespace, source)
            .resolve_key(key)?
            .map(|entry| entry.version_id),
    )
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
    provenance: OperationContext<'_>,
    namespace: NamespaceMutation<'_>,
) -> Result<()> {
    let OperationContext {
        principal,
        operation_id: operation,
    } = provenance;
    let NamespaceMutation { base, fresh } = namespace;
    let Some(repository) =
        repository::main_for_branch(state, branch).map(|r| r.repository_id.clone())
    else {
        return Ok(());
    };
    let origin = {
        let draft = crate::namespace::DraftSource {
            fresh: &*fresh,
            base,
        };
        context_at_key(state, &repository, key, &draft)?.and_then(|c| c.inherited_from)
    };
    let Some(origin) = origin else {
        return Ok(());
    };
    let head = required(state.branches.get(branch), "repository main branch missing")?;
    let head_commit = required(
        state.commits.get(&head.head_commit_id),
        "repository head commit missing",
    )?;
    let original = {
        let draft = crate::namespace::DraftSource {
            fresh: &*fresh,
            base,
        };
        crate::namespace::NamespaceView::new(&head_commit.namespace, &draft)
            .resolve_key(key)?
            .filter(|e| e.kind == NamespaceEntryKind::File)
    }
    .ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileNotFound,
            "inherited file missing",
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
        fresh,
    )?;
    let id = result.branch_id.as_str();
    let inherited_node = {
        let draft = crate::namespace::DraftSource {
            fresh: &*fresh,
            base,
        };
        context_at_key(state, &repository, key, &draft)?.and_then(|c| c.node_id)
    };
    state
        .branches
        .get_mut(id)
        .expect("invariant: file branch creation inserted its generated branch")
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

pub(crate) fn record_directory_edits(
    state: &mut State,
    repository: &str,
    changed_paths: &[String],
    exclude: Option<&str>,
    base: &dyn crate::namespace::NamespaceSource,
    fresh: &mut crate::namespace::NamespaceObjects,
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
        let draft_snapshot = {
            let draft = crate::namespace::DraftSource {
                fresh: &*fresh,
                base,
            };
            let view = match scope_view(state, repository, &path, &draft) {
                Err(e) if e.code() == storage_contract::ErrorCode::DirectoryNotFound => continue,
                Err(e) => return Err(e),
                Ok(view) => view,
            };
            // Resolve the scope once, then materialize its frozen snapshot.
            // An existence check must not enumerate the same subtree first.
            DirectorySnapshotSource::capture(state, view, &draft)?
        };
        result.push(snapshot_source(
            state,
            repository,
            draft_snapshot,
            base,
            fresh,
        )?);
    }
    Ok(result)
}

/// Replace the complete namespace and all frozen descendant references. Merely
/// selecting a directory never advances the selected branch's immutable head.
pub(crate) fn select_change(
    state: &mut State,
    repository: &str,
    branch_id: &str,
    provenance: OperationContext<'_>,
    max_entries: usize,
    namespace: NamespaceMutation<'_>,
) -> Result<Value> {
    let OperationContext {
        principal,
        operation_id: operation,
    } = provenance;
    let NamespaceMutation { base, fresh } = namespace;
    let selected = state
        .directory_branches
        .get(branch_id)
        .cloned()
        .filter(|b| b.repository_id == repository)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::DirectoryBranchNotFound,
                "directory composition missing",
            )
        })?;
    let repo = required(
        state.repositories.get(repository),
        "directory repository missing",
    )?
    .clone();
    let main = required(
        state.branches.get(&repo.main_branch_id),
        "repository main branch missing",
    )?
    .clone();
    let previous = required(
        state.commits.get(&main.head_commit_id),
        "repository head commit missing",
    )?
    .clone();
    required(
        state.directory_trees.get(&selected.head_tree_id),
        "selected directory head tree missing",
    )?;
    required(
        state.commits.get(&selected.commit_id),
        "selected directory commit missing",
    )?;
    for reference in selected.directory_refs.values() {
        required(
            state.directory_branches.get(&reference.directory_branch_id),
            "snapshot directory branch missing",
        )?;
        required(
            state.directory_trees.get(&reference.tree_id),
            "snapshot directory tree missing",
        )?;
    }
    let path = if selected.directory_id == repository {
        String::new()
    } else {
        // Directory identities have no recorded location: one early-stopping
        // walk of the head finds the selection target.
        let mut found = None;
        crate::namespace::NamespaceView::new(&previous.namespace, base).walk(
            |location, entry| {
                if let storage_contract::NamespacePageEntry::Directory { entry_id, .. } = entry
                    && entry_id == &selected.directory_id
                {
                    found = Some(location.as_str().to_owned());
                    return Ok(false);
                }
                Ok(true)
            },
        )?;
        found.ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::DirectoryNotFound,
                "directory no longer exists",
            )
        })?
    };
    let (_, old_entries, _) = scope(state, repository, &path, base)?;
    let restored = rebase_entries(&selected.entries, &selected.path, &path)?;
    let selected_contexts = selected
        .entries
        .iter()
        .zip(&restored)
        .filter(|(_, e)| e.kind == NamespaceEntryKind::File)
        .map(|(old, e)| {
            let context = frozen_context(&selected.file_contexts, &selected.file_branches, old)?;
            Ok((context_key_from_key(repository, e.lookup_key()), context))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    for (file, branch) in &selected.file_branches {
        if !state
            .branches
            .get(branch)
            .is_some_and(|b| b.file_id.as_deref() == Some(file) && b.state == BranchState::Active)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBranchNotFound,
                "snapshot references an unavailable public file branch",
            ));
        }
    }
    let contexts_match = selected_contexts
        .iter()
        .all(|(key, value)| state.file_contexts.get(key) == Some(value));
    if state
        .directory_selections
        .get(&selected.directory_id)
        .is_some_and(|id| id == branch_id)
        && state.directory_tree_selections.get(&selected.directory_id)
            == Some(&selected.head_tree_id)
        && contexts_match
        && old_entries == restored
    {
        return Ok(
            json!({"directory_branch_id":branch_id,"branch_id":repo.main_branch_id,"commit_id":previous.commit_id,"no_change":true,"count":selected.file_branches.len().to_string()}),
        );
    }
    // A frozen subtree can fit by itself while exceeding the repository budget
    // when combined with the currently selected siblings. Check the complete
    // replacement before changing any selections or publishing a commit.
    // A selection replaces a whole subtree: materialize the head once at the
    // operation's own O(subtree + siblings) cost, swap, and rebuild.
    let key = name_policy::path_key(&path);
    let mut entries =
        crate::namespace::NamespaceView::new(&previous.namespace, base).collect_entries()?;
    entries.retain(|e| !key.is_empty() && !namespace::is_descendant(e.lookup_key(), &key));
    entries.extend(restored);
    validate_entries(&mut entries, max_entries)?;

    for e in old_entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
    {
        state
            .file_contexts
            .remove(&context_key_from_key(repository, e.lookup_key()));
    }
    state.file_contexts.extend(selected_contexts);
    let old_ids = old_entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::Directory)
        .map(|e| e.entry_id.clone())
        .collect::<Vec<_>>();
    for id in old_ids {
        state.directory_selections.remove(&id);
        state.directory_tree_selections.remove(&id);
    }
    for e in old_entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
    {
        state.file_write_origins.remove(&e.file_id);
        state.file_selections.remove(&e.file_id);
    }
    state.file_selections.extend(selected.file_branches.clone());
    state
        .file_write_origins
        .extend(selected.inherited_files.clone());
    for (id, reference) in &selected.directory_refs {
        state
            .directory_selections
            .insert(id.clone(), reference.directory_branch_id.clone());
        state
            .directory_tree_selections
            .insert(id.clone(), reference.tree_id.clone());
    }
    state
        .directory_selections
        .insert(selected.directory_id.clone(), branch_id.into());
    state
        .directory_tree_selections
        .insert(selected.directory_id.clone(), selected.head_tree_id.clone());
    let pending = {
        let draft = crate::namespace::DraftSource {
            fresh: &*fresh,
            base,
        };
        snapshot_pending(state, &entries, &draft)?
    };
    let commit_id = new_id("commit");
    state.commits.insert(
        commit_id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: commit_id.clone(),
            history_tree_id: repo.history_tree_id.clone(),
            kind: "directory_selection".into(),
            parent_commit_ids: vec![previous.commit_id],
            tree_id: new_id("tree"),
            sequence: state.sequence,
            operation_id: operation,
            author: principal.into(),
            message: format!("Select directory branch {}", selected.name),
            namespace: crate::namespace::build_from_entries(&entries, fresh)?,
            pending_file_versions: pending,
            provenance: Some(selected.commit_id.clone()),
        },
    );
    let target = state
        .branches
        .get_mut(&repo.main_branch_id)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "repository main branch missing",
            )
        })?;
    target.head_commit_id = commit_id.clone();
    target.generation = target.generation.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "branch generation exhausted",
        )
    })?;
    let generation = state
        .directory_selection_generations
        .entry(selected.directory_id.clone())
        .or_default();
    *generation = generation.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "directory selection generation exhausted",
        )
    })?;
    let changed = if path.is_empty() {
        Vec::new()
    } else {
        vec![path.clone()]
    };
    let parents = record_directory_edits(state, repository, &changed, Some(&key), base, fresh)?;
    Ok(
        json!({"directory_branch_id":branch_id,"branch_id":repo.main_branch_id,"commit_id":commit_id,"history_tree_id":repo.history_tree_id,"path":path,"name":selected.name,"head_tree_id":selected.head_tree_id,"count":selected.file_branches.len().to_string(),"selected":true,"directory_branch_ids":parents}),
    )
}

impl Engine {
    pub fn file_context(&self, repository: &str, path: &str) -> Result<Option<FileContext>> {
        self.with_namespace_source(|source| {
            context_at_key(
                &self.state,
                repository,
                &name_policy::path_key(path),
                source,
            )
        })
    }

    /// Resolve an entry already borrowed from this repository's current main
    /// commit. The caller must not pass an entry from another snapshot. Unlike
    /// `file_context`, this avoids looking the same entry up by path again.
    pub fn file_context_for_current_entry<'a>(
        &'a self,
        repository: &str,
        entry: &Entry,
    ) -> Option<std::borrow::Cow<'a, FileContext>> {
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
            |engine, state, scratch| {
                engine.with_namespace_source(|base| {
                    let repo = state.repositories.get(repository).ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::RepositoryNotFound,
                            "repository missing",
                        )
                    })?;

                    let view = scope_view_for_key(
                        state,
                        repo,
                        Cow::Borrowed(path.map_or("", name_policy::LookupPath::key)),
                        base,
                    )?;
                    let id = view.directory_id.to_owned();
                    if !state.directory_selections.contains_key(&id) {
                        let source = DirectorySnapshotSource::capture(state, view, base)?;
                        snapshot_source(state, repository, source, base, &mut scratch.namespace)?;
                    }
                    let (branch, tree) = selected_tree(state, &id)?;
                    Ok(json!({"directory_branch_id":branch,"head_tree_id":tree.tree_id}))
                })
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
        self.mutate(
            "directory.branches.create",
            principal,
            operation,
            json!({
                "repository_id": repository,
                "path": path.map_or("", name_policy::LookupPath::as_str),
                "name": name.as_str(),
            }),
            |engine, state, scratch| {
                let repo = state.repositories.get(repository).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::RepositoryNotFound,
                        "repository missing",
                    )
                })?;

                let history_tree_id = repo.history_tree_id.clone();
                let directory_id = engine.with_namespace_source(|base| {
                    let repo = required(
                        state.repositories.get(repository),
                        "directory repository missing",
                    )?;
                    let view = scope_view_for_key(
                        state,
                        repo,
                        Cow::Borrowed(path.map_or("", name_policy::LookupPath::key)),
                        base,
                    )?;
                    let directory_id = view.directory_id.to_owned();
                    if !state.directory_selections.contains_key(&directory_id) {
                        let source = DirectorySnapshotSource::capture(state, view, base)?;
                        snapshot_source(state, repository, source, base, &mut scratch.namespace)?;
                    }
                    Ok(directory_id)
                })?;
                if state.directory_branches.values().any(|b| {
                    b.directory_id == directory_id
                        && name_policy::lookup_key(&b.name) == name.key()
                }) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::NameCollision,
                        "directory branch name already exists",
                    ));
                }
                let (parent, tree) = selected_tree(state, &directory_id)?;
                let id = new_id("dbr");
                let inherited_files = tree
                    .file_branches
                    .keys()
                    .map(|file| (file.clone(), id.clone()))
                    .collect();
                let mut file_contexts = BTreeMap::new();
                let mut files = Vec::new();
                for entry in tree
                    .entries
                    .iter()
                    .filter(|e| e.kind == NamespaceEntryKind::File)
                {
                    let mut context =
                        frozen_context(&tree.file_contexts, &tree.file_branches, entry)?;
                    context.selection_generation = 0;
                    context.inherited_from = Some(id.clone());
                    file_contexts.insert(entry.lookup_key().to_owned(), context);
                    let file_branch = required(
                        tree.file_branches.get(&entry.file_id),
                        "snapshot file branch reference missing",
                    )?;
                    files.push(json!({"file_id":entry.file_id,"branch_id":file_branch,
                        "path":entry.path(),"version_id":entry.version_id}));
                }
                let branch = DirectoryBranch {
                    directory_branch_id: id.clone(),
                    repository_id: repository.into(),
                    directory_id,
                    path: tree.path.clone(),
                    name: name.as_str().into(),
                    parent_branch_id: Some(parent.into()),
                    head_tree_id: tree.tree_id.clone(),
                    commit_id: tree.commit_id.clone(),
                    entries: tree.entries.clone(),
                    file_contexts,
                    file_branches: tree.file_branches.clone(),
                    inherited_files,
                    directory_refs: tree.directory_refs.clone(),
                    materialized: false,
                    sequence: state.sequence,
                };
                let tree_id = branch.head_tree_id.clone();
                state.directory_branches.insert(id.clone(), branch);
                Ok(
                    json!({"directory_branch_id":id,"history_tree_id":history_tree_id,
                    "path":path.map_or("",name_policy::LookupPath::as_str),"name":name.as_str(),"branches":files,"copied_files":"0",
                    "placeholder_files":"0","count":files.len().to_string(),
                    "materialized":false,"head_tree_id":tree_id,"selected":false}),
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
        self.mutate(
            "directory.branches.select",
            principal,
            operation,
            json!({
                "repository_id": repository,
                "path": path.map_or("", name_policy::LookupPath::as_str),
                "name": name.as_str(),
            }),
            |engine, state, scratch| {
                engine.with_namespace_source(|base| {
                    let repo = state.repositories.get(repository).ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::RepositoryNotFound,
                            "repository missing",
                        )
                    })?;

                    let view = scope_view_for_key(
                        state,
                        repo,
                        Cow::Borrowed(path.map_or("", name_policy::LookupPath::key)),
                        base,
                    )?;
                    let id = view.directory_id;
                    let selected = state
                        .directory_branches
                        .values()
                        .find(|b| {
                            b.directory_id == id && name_policy::lookup_key(&b.name) == name.key()
                        })
                        .map(|b| b.directory_branch_id.clone())
                        .ok_or_else(|| {
                            Error::new(
                                storage_contract::ErrorCode::DirectoryBranchNotFound,
                                "directory branch missing",
                            )
                        })?;
                    select_change(
                        state,
                        repository,
                        &selected,
                        OperationContext {
                            principal,
                            operation_id: operation,
                        },
                        engine.config.max_entries,
                        NamespaceMutation {
                            base,
                            fresh: &mut scratch.namespace,
                        },
                    )
                })
            },
        )
    }
    pub fn directory_branches(
        &self,
        repository: &str,
        path: &str,
    ) -> Result<Vec<storage_contract::DirectoryBranchSummary>> {
        let view = self.with_namespace_source(|source| {
            let view = scope_view(&self.state, repository, path, source)?;
            Ok(view.directory_id)
        })?;
        Ok(storage_contract::directory_branch_summaries(
            &self.state,
            storage_contract::DirectoryBranchScope {
                repository_id: repository,
                directory_id: &view,
                path,
            },
        ))
    }
    pub fn select_default_directory_branch(
        &mut self,
        repository: &str,
        branch: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "directory.default.select",
            principal,
            operation,
            json!({"repository_id":repository,"directory_branch_id":branch}),
            |engine, state, scratch| {
                state.repositories.get(repository).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::RepositoryNotFound,
                        "repository missing",
                    )
                })?;

                if !state
                    .directory_branches
                    .get(branch)
                    .is_some_and(|b| b.repository_id == repository && b.directory_id == repository)
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::DirectoryBranchScope,
                        "default requires a root composition",
                    ));
                }
                engine.with_namespace_source(|base| {
                    select_change(
                        state,
                        repository,
                        branch,
                        OperationContext {
                            principal,
                            operation_id: operation,
                        },
                        engine.config.max_entries,
                        NamespaceMutation {
                            base,
                            fresh: &mut scratch.namespace,
                        },
                    )
                })
            },
        )
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
            |_, state, _| {
                state.repositories.get(repository).ok_or_else(|| {
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
