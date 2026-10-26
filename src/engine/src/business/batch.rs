//! Atomic branch-scoped namespace edits. All baselines are checked before any
//! publication; named branches retain their identities and gain subtree nodes.
use super::*;
use crate::engine::Prepared;
use json_codec::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct CopiedFile {
    entry_id: String,
    file_id: String,
    version_id: String,
    size: u64,
}

fn contains(scope: &str, path: &str) -> bool {
    let scope = name_policy::path_key(scope);
    let path = name_policy::path_key(path);
    scope.is_empty() || namespace::is_descendant(&path, &scope)
}

/// Active compositions are published by complete subtree replacement. Parent
/// and child drafts cannot both replace the live view without discarding one
/// draft's changes or mixing their independently frozen contexts. Reject that
/// unsupported combination before creating any nodes or preparing payloads.
fn active_scopes(
    state: &State,
    originals: &BTreeMap<String, DirectoryBranch>,
    touched: &BTreeSet<String>,
    repository: &str,
    current: &[Entry],
) -> Result<Vec<String>> {
    let mut active = Vec::new();
    let mut paths = Vec::<String>::new();
    for id in touched {
        let branch = &originals[id];
        if state.directory_selections.get(&branch.directory_id) != Some(id)
            || state
                .directory_tree_selections
                .get(&branch.directory_id)
                .unwrap_or(&branch.head_tree_id)
                != &branch.head_tree_id
        {
            continue;
        }
        let path = if branch.directory_id == repository {
            String::new()
        } else {
            current
                .iter()
                .find(|entry| {
                    entry.kind == NamespaceEntryKind::Directory
                        && entry.entry_id == branch.directory_id
                })
                .map(|entry| entry.lookup_key.clone())
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::DirectoryBranchConflict,
                        "active directory is missing",
                    )
                })?
        };
        if paths
            .iter()
            .any(|other| path == *other || contains(other, &path) || contains(&path, other))
        {
            return Err(Error::new(
                storage_contract::ErrorCode::BatchScopeConflict,
                "overlapping active directory scopes must use one common ancestor composition",
            ));
        }
        paths.push(path);
        active.push(id.clone());
    }
    Ok(active)
}
fn remove(branch: &mut DirectoryBranch, source: &Entry) {
    let key = &source.lookup_key;
    let removed = branch
        .entries
        .iter()
        .filter(|e| e.lookup_key == *key || namespace::is_descendant(&e.lookup_key, key))
        .cloned()
        .collect::<Vec<_>>();
    branch
        .entries
        .retain(|e| e.lookup_key != *key && !namespace::is_descendant(&e.lookup_key, key));
    for e in removed {
        if e.kind == NamespaceEntryKind::File {
            branch.file_contexts.remove(&e.lookup_key);
            if let Some(remaining) = branch
                .entries
                .iter()
                .find(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == e.file_id)
            {
                if let Some(context) = branch.file_contexts.get(&remaining.lookup_key) {
                    branch
                        .file_branches
                        .insert(e.file_id.clone(), context.branch_id.clone());
                    if let Some(origin) = &context.inherited_from {
                        branch
                            .inherited_files
                            .insert(e.file_id.clone(), origin.clone());
                    } else {
                        branch.inherited_files.remove(&e.file_id);
                    }
                }
            } else {
                branch.file_branches.remove(&e.file_id);
                branch.inherited_files.remove(&e.file_id);
            }
        } else {
            branch.directory_refs.remove(&e.entry_id);
        }
    }
}
fn relocate(entry: &Entry, source: &str, target: &str) -> Entry {
    let depth = source.split('/').count();
    let mut e = entry.clone();
    let suffix = e.path.split('/').skip(depth).collect::<Vec<_>>().join("/");
    e.path = if suffix.is_empty() {
        target.into()
    } else {
        format!("{target}/{suffix}")
    };
    e.name = e.path.rsplit('/').next().unwrap_or("").into();
    e.lookup_key = name_policy::path_key(&e.path);
    e
}
fn validate_identities(entries: &[Entry]) -> Result<()> {
    let mut dirs = BTreeSet::new();
    for e in entries
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::Directory)
    {
        if !dirs.insert(&e.entry_id) {
            return Err(Error::new(
                storage_contract::ErrorCode::BatchScopeConflict,
                "duplicate directory identity in composed namespace",
            ));
        }
    }
    Ok(())
}

// Frozen compositions require exact path context, not a reconstructed owner.
fn file_context(branch: &DirectoryBranch, entry: &Entry) -> Result<FileContext> {
    branch.file_branches.get(&entry.file_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "source public file branch missing",
        )
    })?;
    branch
        .file_contexts
        .get(&entry.lookup_key)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "source path write context missing",
            )
        })
}

fn save_inactive(
    state: &mut State,
    branch: &DirectoryBranch,
    principal: &str,
    operation: &str,
    maximum: usize,
    namespace_deletions: Option<NamespaceDeletions>,
) -> Result<()> {
    let previous = state
        .directory_branches
        .get(&branch.directory_branch_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory branch missing",
            )
        })?;
    let mut entries = branch.entries.clone();
    // Snapshot commits include real ancestor identities so the normal namespace
    // validator and portable history export do not need a second path model.
    let base = state.commits.get(&previous.commit_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "directory branch commit missing",
        )
    })?;
    let key = name_policy::path_key(&branch.path);
    entries.extend(
        base.entries
            .iter()
            .filter(|e| {
                e.kind == NamespaceEntryKind::Directory
                    && (e.lookup_key == key || namespace::is_descendant(&key, &e.lookup_key))
            })
            .cloned(),
    );
    validate_entries(&mut entries, maximum)?;
    let commit_id = new_id("commit");
    let repo = state
        .repositories
        .get(&branch.repository_id)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory repository missing",
            )
        })?;
    state.commits.insert(
        commit_id.clone(),
        Commit {
            namespace_deletions,
            commit_id: commit_id.clone(),
            history_tree_id: repo.history_tree_id.clone(),
            kind: "directory_namespace".into(),
            parent_commit_ids: vec![previous.commit_id],
            tree_id: new_id("tree"),
            sequence: state.sequence,
            operation_id: operation.into(),
            author: principal.into(),
            message: "Branch-scoped namespace edit".into(),
            entries,
            pending_file_versions: BTreeMap::new(),
            provenance: None,
        },
    );
    let tree_id = new_id("dtree");
    state.directory_trees.insert(
        tree_id.clone(),
        DirectoryTree {
            tree_id: tree_id.clone(),
            directory_branch_id: branch.directory_branch_id.clone(),
            parent_tree_id: Some(previous.head_tree_id),
            commit_id: commit_id.clone(),
            path: branch.path.clone(),
            entries: branch.entries.clone(),
            file_contexts: branch.file_contexts.clone(),
            file_branches: branch.file_branches.clone(),
            inherited_files: branch.inherited_files.clone(),
            directory_refs: branch.directory_refs.clone(),
            sequence: state.sequence,
        },
    );
    let mut updated = branch.clone();
    updated.head_tree_id = tree_id;
    updated.commit_id = commit_id;
    updated.materialized = true;
    updated.sequence = state.sequence;
    state
        .directory_branches
        .insert(updated.directory_branch_id.clone(), updated);
    Ok(())
}

fn apply(
    engine: &Engine,
    state: &mut State,
    pending: &mut Vec<Prepared>,
    repository: &str,
    request: &DirectoryNamespaceBatch,
    principal: &str,
    operation: &str,
) -> Result<Value> {
    let repo = state.repositories.get(repository).cloned().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::RepositoryNotFound,
            "repository missing",
        )
    })?;

    if !["move", "copy", "delete", "rename"].contains(&request.kind.as_str())
        || request.items.is_empty()
        || request.items.len() > engine.config.max_entries
    {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            "batch requires supported kind and a bounded nonempty item list",
        ));
    }
    let mut originals = BTreeMap::new();
    for item in &request.items {
        name_policy::validate_path(&item.source.path)?;
        let mut refs = vec![(
            &item.source.directory_branch_id,
            &item.source.expected_tree_id,
        )];
        if let Some(target) = &item.target {
            name_policy::validate_path(&target.path)?;
            refs.push((&target.directory_branch_id, &target.expected_tree_id));
        }
        if (request.kind == "delete") != item.target.is_none() {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "only delete omits a destination",
            ));
        }
        for (id, expected) in refs {
            let branch = state
                .directory_branches
                .get(id)
                .filter(|b| b.repository_id == repository)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::DirectoryBranchNotFound,
                        "batch branch missing from repository",
                    )
                })?;
            if &branch.head_tree_id != expected {
                return Err(Error::new(
                    storage_contract::ErrorCode::DirectoryBranchConflict,
                    "batch subtree baseline changed; no item was published",
                ));
            }
            originals.insert(id.clone(), branch.clone());
        }
    }
    let main = state
        .branches
        .get(&repo.main_branch_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "repository main branch missing",
            )
        })?;
    let current = state
        .commits
        .get(&main.head_commit_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "repository head missing",
            )
        })?;
    let mut drafts = originals.clone();
    let mut touched = BTreeSet::new();
    let mut source_ranges = BTreeMap::<String, Vec<String>>::new();
    let mut mappings = BTreeSet::new();
    let mut planned = Vec::new();
    let mut deleted = Vec::<(String, DeletedPath)>::new();
    for item in &request.items {
        let source_scope = &originals[&item.source.directory_branch_id];
        if !contains(&source_scope.path, &item.source.path) {
            return Err(Error::new(
                storage_contract::ErrorCode::DirectoryBranchScope,
                "source path outside selected scope",
            ));
        }
        let source = namespace::find_unordered(&source_scope.entries, &item.source.path)
            .cloned()
            .filter(|e| e.entry_id == item.source.entry_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::EntryNotFound,
                    "source identity/path does not match frozen subtree",
                )
            })?;
        let ranges = source_ranges
            .entry(source_scope.directory_branch_id.clone())
            .or_default();
        if ranges.iter().any(|key| {
            *key != source.lookup_key
                && (namespace::is_descendant(key, &source.lookup_key)
                    || namespace::is_descendant(&source.lookup_key, key))
        }) {
            return Err(Error::new(
                storage_contract::ErrorCode::BatchScopeConflict,
                "overlapping source entries must be selected once",
            ));
        }
        ranges.push(source.lookup_key.clone());
        let target_key = item
            .target
            .as_ref()
            .map(|t| {
                format!(
                    "{}:{}",
                    t.directory_branch_id,
                    name_policy::path_key(&t.path)
                )
            })
            .unwrap_or_default();
        if !mappings.insert((
            source_scope.directory_branch_id.clone(),
            source.entry_id.clone(),
            source.lookup_key.clone(),
            target_key,
        )) {
            return Err(Error::new(
                storage_contract::ErrorCode::BatchMappingConflict,
                "duplicate batch mapping",
            ));
        }
        if request.kind == "rename"
            && item.target.as_ref().is_some_and(|t| {
                t.directory_branch_id == source_scope.directory_branch_id && t.path == source.path
            })
        {
            continue;
        }
        if request.kind == "delete" {
            // Capture only actual deletions, never the temporary removals used
            // to plan moves or renames. Keep request and source-entry order.
            for entry in &drafts[&source_scope.directory_branch_id].entries {
                if entry.lookup_key == source.lookup_key
                    || namespace::is_descendant(&entry.lookup_key, &source.lookup_key)
                {
                    deleted.push((
                        source_scope.directory_branch_id.clone(),
                        namespace::deleted_path(entry),
                    ));
                }
            }
        }
        if request.kind != "copy" {
            remove(
                drafts
                    .get_mut(&source_scope.directory_branch_id)
                    .expect("checked"),
                &source,
            );
            touched.insert(source_scope.directory_branch_id.clone());
        }
        let Some(target) = &item.target else {
            continue;
        };
        let target_original = &originals[&target.directory_branch_id];
        if !contains(&target_original.path, &target.path) {
            return Err(Error::new(
                storage_contract::ErrorCode::DirectoryBranchScope,
                "destination outside selected scope",
            ));
        }
        if request.kind == "rename"
            && target.directory_branch_id != source_scope.directory_branch_id
        {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "rename preserves its directory branch; use move for transfer",
            ));
        }
        if source.kind == NamespaceEntryKind::Directory
            && namespace::is_descendant(&name_policy::path_key(&target.path), &source.lookup_key)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidMove,
                "cannot move/copy directory inside itself",
            ));
        }
        touched.insert(target.directory_branch_id.clone());
        planned.push((item, source));
    }
    // Every source was resolved against its original frozen composition, and
    // all non-copy sources are now absent from drafts. Check the complete
    // destination namespace before creating path nodes or preparing content.
    let mut projected = touched
        .iter()
        .map(|id| (id.clone(), drafts[id].entries.clone()))
        .collect::<BTreeMap<_, _>>();
    for (item, source) in &planned {
        let source_scope = &originals[&item.source.directory_branch_id];
        let target = item.target.as_ref().expect("planned destination");
        let target_scope = &originals[&target.directory_branch_id];
        let entries = projected
            .get_mut(&target.directory_branch_id)
            .expect("checked");
        if namespace::find_unordered(entries, &target.path).is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::NameCollision,
                "batch destination exists",
            ));
        }
        // A destination must have a real parent after all sources are removed.
        if let Some((parent, _)) = target.path.rsplit_once('/')
            && name_policy::path_key(parent) != name_policy::path_key(&target_scope.path)
            && !namespace::find_unordered(entries, parent)
                .is_some_and(|e| e.kind == NamespaceEntryKind::Directory)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::DirectoryNotFound,
                "destination parent missing in selected subtree",
            ));
        }
        entries.extend(
            source_scope
                .entries
                .iter()
                .filter(|entry| {
                    entry.lookup_key == source.lookup_key
                        || namespace::is_descendant(&entry.lookup_key, &source.lookup_key)
                })
                .map(|entry| relocate(entry, &source.path, &target.path)),
        );
    }
    for id in &touched {
        let mut full = projected.remove(id).expect("checked");
        ensure_parents_for_scope(state, &drafts[id], &mut full)?;
        validate_entries(&mut full, engine.config.max_entries)?;
    }
    let active = active_scopes(state, &originals, &touched, repository, &current.entries)?;
    let active_rename_changes = request
        .items
        .iter()
        .filter_map(|item| {
            let source = &originals[&item.source.directory_branch_id];
            let target = item.target.as_ref()?;
            if request.kind != "rename" && request.kind != "move" {
                return None;
            }
            if state.directory_selections.get(&source.directory_id)
                != Some(&source.directory_branch_id)
            {
                return None;
            }
            if state
                .directory_tree_selections
                .get(&source.directory_id)
                .unwrap_or(&source.head_tree_id)
                != &source.head_tree_id
            {
                return None;
            }
            let entry = find_unordered(&current.entries, &item.source.path)?;
            if entry.entry_id != item.source.entry_id {
                return None;
            }
            Some(NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: item.source.path.clone(),
                destination: Some(target.path.clone()),
            })
        })
        .collect::<Vec<_>>();
    let renamed_directories = directory::prepare_independent_renamed_directories(
        state,
        repository,
        &current.entries,
        &active_rename_changes,
    )?;
    let baseline_heads = state
        .directory_branches
        .iter()
        .map(|(id, b)| (id.clone(), b.head_tree_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut owners = BTreeMap::<String, String>::new();
    let mut copies = BTreeMap::<(String, String, String, String), CopiedFile>::new();
    let mut directory_copies = BTreeMap::<(String, String, String), String>::new();
    let mut copy_receipts = Vec::new();
    for (item, source) in planned {
        let source_scope = &originals[&item.source.directory_branch_id];
        let target = item.target.as_ref().expect("planned destination");
        let target_draft = drafts
            .get_mut(&target.directory_branch_id)
            .expect("checked");
        let members = source_scope
            .entries
            .iter()
            .filter(|e| {
                e.lookup_key == source.lookup_key
                    || namespace::is_descendant(&e.lookup_key, &source.lookup_key)
            })
            .cloned()
            .collect::<Vec<_>>();
        for member in members {
            let mut moved = relocate(&member, &source.path, &target.path);
            let parent_id = if let Some((parent, _)) = moved.path.rsplit_once('/') {
                if name_policy::path_key(parent) == name_policy::path_key(&target_draft.path) {
                    target_draft.directory_id.clone()
                } else {
                    find_unordered(&target_draft.entries, parent)
                        .map(|e| e.entry_id.clone())
                        .ok_or_else(|| {
                            Error::new(
                                storage_contract::ErrorCode::DirectoryNotFound,
                                "destination parent missing",
                            )
                        })?
                }
            } else {
                target_draft.directory_id.clone()
            };
            if request.kind == "copy" {
                if moved.kind == NamespaceEntryKind::Directory {
                    let key = (
                        member.entry_id.clone(),
                        parent_id.clone(),
                        name_policy::lookup_key(&moved.name),
                    );
                    if let Some(previous) = directory_copies.get(&key) {
                        moved.entry_id = previous.clone();
                    } else {
                        moved.entry_id = new_id("entry");
                        directory_copies.insert(key, moved.entry_id.clone());
                    }
                } else {
                    let cache_key = (
                        member.file_id.clone(),
                        member.version_id.clone(),
                        parent_id.clone(),
                        name_policy::lookup_key(&moved.name),
                    );
                    if let Some(existing) = copies.get(&cache_key) {
                        // Frozen target branches can name the same directory
                        // differently. Share only the new copy's identity and
                        // body, never another target's logical path spelling.
                        moved.entry_id = existing.entry_id.clone();
                        moved.file_id = existing.file_id.clone();
                        moved.version_id = existing.version_id.clone();
                        moved.size = existing.size;
                    } else {
                        let new_file = new_id("file");
                        let (source_path, original) = engine.valid_replica(&member.version_id)?;
                        let (primary, count) =
                            engine.new_file_placement(repository, &moved.path)?;
                        let version = engine.build_copied_version(
                            state,
                            pending,
                            crate::business::VersionBuild {
                                history: &repo.history_tree_id,
                                file_id: &new_file,
                                path: &moved.path,
                                source: &source_path,
                                primary: &primary,
                                copies: count,
                                source_ids: Vec::new(),
                                checkpoint: true,
                            },
                            &original,
                        )?;
                        moved.entry_id = new_id("entry");
                        moved.file_id = new_file;
                        moved.version_id = version.version_id;
                        moved.size = version.logical_size;
                        copy_receipts.push(json!({"source_file_id":member.file_id,"source_version_id":member.version_id,"file_id":moved.file_id,"version_id":moved.version_id,"path":moved.path}));
                        copies.insert(
                            cache_key,
                            CopiedFile {
                                entry_id: moved.entry_id.clone(),
                                file_id: moved.file_id.clone(),
                                version_id: moved.version_id.clone(),
                                size: moved.size,
                            },
                        );
                    }
                    let mut commit = state
                        .commits
                        .get(&source_scope.commit_id)
                        .cloned()
                        .ok_or_else(|| {
                            Error::new(
                                storage_contract::ErrorCode::StateInvalid,
                                "source directory commit missing",
                            )
                        })?;
                    // The source snapshot supplies ancestry, not authorship:
                    // initializing this independent file belongs to this copy.
                    commit.operation_id = operation.into();
                    commit.author = principal.into();
                    commit.entries = target_draft.entries.clone();
                    commit.entries.push(moved.clone());
                    ensure_parents_for_scope(state, target_draft, &mut commit.entries)?;
                    validate_entries(&mut commit.entries, engine.config.max_entries)?;
                    let branch = file::ensure_shared_file_branch(state, &commit, &moved)?;
                    target_draft.file_contexts.insert(
                        moved.lookup_key.clone(),
                        FileContext {
                            node_id: None,
                            selection_generation: 0,
                            branch_id: branch.clone(),
                            inherited_from: None,
                        },
                    );
                    target_draft
                        .file_branches
                        .insert(moved.file_id.clone(), branch);
                    // The first target created the body, placement and copy
                    // receipt. Later logical aliases must not relocate it.
                    state
                        .file_locations
                        .entry(moved.file_id.clone())
                        .or_insert_with(|| moved.path.clone());
                    state
                        .file_names
                        .entry(moved.file_id.clone())
                        .or_insert_with(|| moved.name.clone());
                }
            } else if moved.kind == NamespaceEntryKind::File {
                let context = file_context(source_scope, &member)?;
                let context = if member.path != moved.path {
                    file_paths::rename_node(
                        state,
                        repository,
                        &member,
                        &moved,
                        Some(context),
                        principal,
                        operation,
                    )?
                } else {
                    context
                };
                target_draft
                    .file_contexts
                    .insert(moved.lookup_key.clone(), context);
                let branch = source_scope
                    .file_branches
                    .get(&member.file_id)
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::StateInvalid,
                            "source public file branch missing",
                        )
                    })?;
                target_draft
                    .file_branches
                    .insert(member.file_id.clone(), branch.clone());
                if let Some(origin) = source_scope.inherited_files.get(&member.file_id) {
                    target_draft
                        .inherited_files
                        .insert(member.file_id.clone(), origin.clone());
                }
                directory::set_namespace_location(
                    state,
                    &member,
                    &source.path,
                    &target.path,
                    &request.kind,
                    source.kind == NamespaceEntryKind::Directory,
                );
                // set_namespace_location above inserts this exact file identity.
                let canonical = state.file_locations[&moved.file_id].clone();
                if let Some(previous) = owners.insert(moved.file_id.clone(), canonical.clone())
                    && name_policy::path_key(&previous) != name_policy::path_key(&canonical)
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::BatchMappingConflict,
                        "one physical file cannot have two move/rename outcomes",
                    ));
                }
            } else if let Some(reference) = source_scope.directory_refs.get(&member.entry_id) {
                target_draft
                    .directory_refs
                    .insert(member.entry_id.clone(), reference.clone());
            }
            target_draft.entries.push(moved);
        }
    }
    if touched.is_empty() {
        return Ok(
            json!({"branch_id":repo.main_branch_id,"history_tree_id":repo.history_tree_id,"commit_id":main.head_commit_id,"no_change":true,"directory_branch_ids":[],"changes":[],"copies":[]}),
        );
    }
    for id in &touched {
        let draft = drafts.get_mut(id).expect("checked");
        let mut full = draft.entries.clone();
        ensure_parents_for_scope(state, draft, &mut full)?;
        validate_entries(&mut full, engine.config.max_entries)?;
        validate_identities(&draft.entries)?;
        sorted(&mut draft.entries);
    }
    let deletion_timestamp = now();
    let active_ids = active.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut active_deleted = Vec::new();
    let mut inactive_deleted = BTreeMap::<String, Vec<DeletedPath>>::new();
    for (scope, path) in deleted {
        if active_ids.contains(scope.as_str()) {
            active_deleted.push((scope, path));
        } else {
            inactive_deleted.entry(scope).or_default().push(path);
        }
    }
    let previous = current;
    let mut entries = previous.entries.clone();
    let mut changed_paths = renamed_directories;
    let mut active_paths = BTreeMap::new();
    for id in &active {
        let draft = &drafts[id];
        let path = if draft.directory_id == repository {
            String::new()
        } else {
            entries
                .iter()
                .find(|e| {
                    e.kind == NamespaceEntryKind::Directory && e.entry_id == draft.directory_id
                })
                .map(|e| e.path.clone())
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::DirectoryBranchConflict,
                        "active directory was removed by overlapping mapping",
                    )
                })?
        };
        active_paths.insert(id.clone(), path.clone());
        let key = name_policy::path_key(&path);
        let removed = entries
            .iter()
            .filter(|e| key.is_empty() || namespace::is_descendant(&e.lookup_key, &key))
            .cloned()
            .collect::<Vec<_>>();
        entries.retain(|e| !key.is_empty() && !namespace::is_descendant(&e.lookup_key, &key));
        for e in &removed {
            if e.kind == NamespaceEntryKind::File {
                state
                    .file_contexts
                    .remove(&directory::context_key(repository, &e.path));
            }
        }
        for e in removed {
            if e.kind == NamespaceEntryKind::File {
                state.file_selections.remove(&e.file_id);
                state.file_write_origins.remove(&e.file_id);
            }
        }
        entries.extend(directory::rebase_entries(
            &draft.entries,
            &draft.path,
            &path,
        )?);
        for e in draft
            .entries
            .iter()
            .filter(|e| e.kind == NamespaceEntryKind::File)
        {
            let context = file_context(draft, e)?;
            let restored = directory::rebase_entries(std::slice::from_ref(e), &draft.path, &path)?;
            state.file_contexts.insert(
                directory::context_key(repository, &restored[0].path),
                context,
            );
        }
        state.file_selections.extend(draft.file_branches.clone());
        state
            .file_write_origins
            .extend(draft.inherited_files.clone());
        changed_paths.push(if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        });
    }
    validate_entries(&mut entries, engine.config.max_entries)?;
    validate_identities(&entries)?;
    let active_deleted = active_deleted
        .into_iter()
        .map(|(scope, mut deleted)| {
            deleted.path =
                directory::rebase_path(&deleted.path, &drafts[&scope].path, &active_paths[&scope])?;
            Ok(deleted)
        })
        .collect::<Result<Vec<_>>>()?;
    let commit_id = if !active.is_empty() {
        let id = new_id("commit");
        let pending = directory::snapshot_pending(state, &entries);
        state.commits.insert(
            id.clone(),
            Commit {
                namespace_deletions: namespace::deletion_record(active_deleted, deletion_timestamp),
                commit_id: id.clone(),
                history_tree_id: repo.history_tree_id.clone(),
                kind: "namespace_batch".into(),
                parent_commit_ids: vec![previous.commit_id],
                tree_id: new_id("tree"),
                sequence: state.sequence,
                operation_id: operation.into(),
                author: principal.into(),
                message: format!("{} {} branch mappings", request.kind, request.items.len()),
                entries,
                pending_file_versions: pending,
                provenance: None,
            },
        );
        let target = state
            .branches
            .get_mut(&repo.main_branch_id)
            .expect("checked");
        target.head_commit_id = id.clone();
        target.generation = target.generation.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "branch generation exhausted",
            )
        })?;
        Some(id)
    } else {
        None
    };
    for id in touched.iter().filter(|id| !active.contains(id)) {
        save_inactive(
            state,
            &drafts[id],
            principal,
            operation,
            engine.config.max_entries,
            namespace::deletion_record(
                inactive_deleted.remove(id).unwrap_or_default(),
                deletion_timestamp,
            ),
        )?;
    }
    let ancestors = directory::record_directory_edits(state, repository, &changed_paths, None)?;
    let mut affected = touched;
    affected.extend(ancestors);
    let changes=affected.iter().map(|id| {let b=&state.directory_branches[id];json!({"directory_branch_id":id,"previous_tree_id":baseline_heads.get(id),"head_tree_id":b.head_tree_id,"path":b.path})}).collect::<Vec<_>>();
    let transfers=request.items.iter().map(|item| {
        let source=&originals[&item.source.directory_branch_id];let e=namespace::find_unordered(&source.entries,&item.source.path).expect("source checked");
        json!({"source":item.source,"target":item.target,"file_id":e.file_id,"version_id":e.version_id,"physical_path":state.file_locations.get(&e.file_id)})
    }).collect::<Vec<_>>();
    Ok(
        json!({"commit_id":commit_id,"branch_id":repo.main_branch_id,"history_tree_id":repo.history_tree_id,"action":request.kind,"count":request.items.len().to_string(),"directory_branch_ids":affected,"changes":changes,"transfers":transfers,"copies":copy_receipts,"file_node_ids":file_paths::operation_nodes(state,operation)}),
    )
}
fn ensure_parents_for_scope(
    state: &State,
    branch: &DirectoryBranch,
    entries: &mut Vec<Entry>,
) -> Result<()> {
    let key = name_policy::path_key(&branch.path);
    entries.extend(
        state
            .commits
            .get(&branch.commit_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "directory branch commit missing",
                )
            })?
            .entries
            .iter()
            .filter(|e| {
                e.kind == NamespaceEntryKind::Directory
                    && (e.lookup_key == key || namespace::is_descendant(&key, &e.lookup_key))
            })
            .cloned(),
    );
    Ok(())
}
impl Engine {
    pub fn directory_namespace_batch(
        &mut self,
        repository: &str,
        request: &DirectoryNamespaceBatch,
        principal: &str,
        operation: &str,
    ) -> Result<OperationResult> {
        self.mutate(
            "directory.namespace.batch",
            principal,
            operation,
            json!({"repository_id":repository,"request":request}),
            |engine, state, pending| {
                apply(
                    engine, state, pending, repository, request, principal, operation,
                )
            },
        )
    }
}
