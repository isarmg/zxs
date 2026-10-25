//! File browsing reuses bounded directory-first ordering and controlled relative
//! paths; published content remains managed by the storage transaction engine.
use super::*;
mod activity;
mod backup_namespace;
mod history_nodes;
mod listing;
mod operations;
mod pagination;
mod scope_query;
pub(super) use scope_query::read as query_directory_branch_scopes;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

/// Present stored UTC seconds in the same whole-second UTC form as body
/// versions. This changes the read projection, never the persisted timestamp.
fn utc_timestamp(seconds: u64) -> Result<String, ApiError> {
    let invalid = || ApiError::new(500, "STATE_INVALID", "history timestamp is invalid");
    date_time::OffsetDateTime::from_unix_timestamp(i64::try_from(seconds).map_err(|_| invalid())?)
        .map_err(|_| invalid())?
        .format(&date_time::format_description::well_known::Rfc3339)
        .map_err(|_| invalid())
}

fn query<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get("query")
        .and_then(|q| q.get(key))
        .and_then(Value::as_str)
}
fn path(input: &Value) -> Result<&str, ApiError> {
    checked_path(input, false)
}
fn lookup_path(input: &Value) -> Result<&str, ApiError> {
    checked_path(input, true)
}
fn checked_path(input: &Value, lookup: bool) -> Result<&str, ApiError> {
    // An omitted path denotes the root, but an explicit malformed value must
    // never silently select it (especially for branch and backup mutations).
    let supplied = input
        .get("path")
        .or_else(|| input.get("query").and_then(|q| q.get("path")));
    let value = supplied
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| ApiError::invalid("path must be a string"))
        })
        .transpose()?
        .unwrap_or("");
    if !value.is_empty() {
        if lookup {
            name_policy::validate_lookup_path(value)
        } else {
            name_policy::validate_path(value)
        }
        .map_err(ApiError::invalid)?;
    }
    Ok(value)
}
fn disk<'a>(engine: &'a Engine, slot: &str) -> Result<&'a storage_engine::DiskConfig, ApiError> {
    engine
        .config()
        .disks
        .iter()
        .find(|d| d.slot_id == slot)
        .ok_or_else(|| ApiError::new(404, "DISK_NOT_FOUND", "disk not found"))
}
fn eligible(engine: &Engine, slot: &str, repo: &Repository) -> bool {
    repo.primary_slot_id == slot
        || engine.state().placements.values().any(|p| {
            engine
                .state()
                .versions
                .get(&p.version_id)
                .is_some_and(|version| version.history_tree_id == repo.history_tree_id)
                && p.route_slot_ids.iter().any(|s| s == slot)
        })
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiskAccess {
    Read,
    Write,
    Create,
}

fn needs_disk_root(engine: &Engine, slot: &str, path: &str) -> bool {
    let first = path.split('/').next().unwrap_or("");
    !engine.state().repositories.values().any(|repo| {
        repo.primary_slot_id == slot
            && (repo.file_manager_root
                || name_policy::lookup_key(&repo.name) == name_policy::lookup_key(first))
    })
}

fn admit_root_operation(
    engine: &Engine,
    slot: &str,
    path: &str,
    operation: &str,
) -> Result<(), ApiError> {
    if needs_disk_root(engine, slot, path) && engine.state().operations.contains_key(operation) {
        return Err(ApiError::conflict(
            "IDEMPOTENCY_KEY_REUSED",
            "operation is already bound to another request",
        ));
    }
    Ok(())
}

/// Named repositories and a disk-root tree share one displayed namespace.
/// A normalized first component must select exactly one owner, for both reads
/// and writes. Explicit repository/entry identities remain independent of it.
fn ensure_unambiguous_disk_path(
    engine: &Engine,
    root: &Repository,
    path: &str,
) -> Result<(), ApiError> {
    let first = path.split('/').next().unwrap_or("");
    let key = name_policy::lookup_key(first);
    let names = engine
        .state()
        .repositories
        .values()
        .filter(|repo| repo.primary_slot_id == root.primary_slot_id && !repo.file_manager_root)
        .map(|repo| name_policy::lookup_key(&repo.name))
        .filter(|name| path.is_empty() || *name == key)
        .collect::<BTreeSet<_>>();
    if !names.is_empty()
        && engine
            .list_tree_ref(&root.main_branch_id)
            .map_err(error)?
            .iter()
            .any(|entry| names.contains(&entry.lookup_key))
    {
        return Err(ApiError::conflict(
            "NAME_COLLISION",
            "a named repository conflicts with a disk-root entry; use an explicit repository identity to rename the entry",
        ));
    }
    Ok(())
}

fn resolve_existing(
    engine: &Engine,
    slot: &str,
    value: &str,
    access: DiskAccess,
) -> Result<Option<(Repository, String)>, ApiError> {
    let write = access != DiskAccess::Read;
    if write && disk(engine, slot)?.level != 1 {
        return Err(ApiError::new(
            403,
            "DISK_READ_ONLY",
            "backup disks are read only",
        ));
    }
    let (primary, relative) = (slot.to_owned(), value);
    let existing = engine
        .state()
        .repositories
        .values()
        .find(|r| r.primary_slot_id == primary && r.file_manager_root)
        .cloned();
    if let Some(root) = &existing {
        ensure_unambiguous_disk_path(engine, root, relative)?;
    }
    let first = relative.split('/').next().unwrap_or("");
    if let Some(repo) = engine
        .state()
        .repositories
        .values()
        .find(|r| {
            r.primary_slot_id == primary
                && !r.file_manager_root
                && name_policy::lookup_key(&r.name) == name_policy::lookup_key(first)
        })
        .cloned()
    {
        if !eligible(engine, slot, &repo) {
            return Err(ApiError::new(
                404,
                "DIRECTORY_NOT_FOUND",
                "directory is not stored on this disk",
            ));
        }
        return Ok(Some((
            repo,
            relative
                .split_once('/')
                .map(|(_, p)| p)
                .unwrap_or("")
                .into(),
        )));
    }
    let Some(repo) = existing else {
        return Ok(None);
    };
    Ok(Some((repo, relative.into())))
}
fn empty_directory() -> ApiError {
    ApiError::new(404, "DIRECTORY_NOT_FOUND", "disk directory is empty")
}

fn resolve_read(
    engine: &Engine,
    slot: &str,
    value: &str,
) -> Result<(Repository, String), ApiError> {
    resolve_existing(engine, slot, value, DiskAccess::Read)?.ok_or_else(empty_directory)
}

fn resolve(
    engine: &mut Engine,
    identity: &Identity,
    slot: &str,
    value: &str,
    access: DiskAccess,
) -> Result<(Repository, String), ApiError> {
    if let Some(resolved) = resolve_existing(engine, slot, value, access)? {
        return Ok(resolved);
    }
    if access != DiskAccess::Create {
        return Err(empty_directory());
    }

    let repo = engine
        .create_disk_root(slot, &identity.principal, &storage_engine::new_id("op"))
        .map_err(error)?;
    Ok((repo, value.into()))
}

fn file_repo<'a>(engine: &'a Engine, file: &str) -> Result<&'a Repository, ApiError> {
    let version = engine
        .state()
        .versions
        .values()
        .find(|v| v.file_id == file)
        .ok_or_else(|| ApiError::new(404, "FILE_NOT_FOUND", "file not found"))?;
    let repo = repo_for_history(engine, &version.history_tree_id)?;
    Ok(repo)
}
#[cfg(test)]
thread_local! { static LISTED_ENTRY_PROJECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn listed_current_entry(
    engine: &Engine,
    repo: &Repository,
    index: usize,
    prefix: &str,
) -> Result<Value, ApiError> {
    #[cfg(test)]
    LISTED_ENTRY_PROJECTIONS.set(LISTED_ENTRY_PROJECTIONS.get() + 1);
    let entry = engine
        .list_tree_ref(&repo.main_branch_id)
        .map_err(error)?
        .get(index)
        .ok_or_else(|| ApiError::new(500, "STATE_INVALID", "listing entry index is invalid"))?;
    let display = if prefix.is_empty() {
        entry.path.clone()
    } else {
        format!("{prefix}/{}", entry.path)
    };
    let context = engine.file_context_for_current_entry(&repo.repository_id, entry);
    let mut response = json!({"entry_id":entry.entry_id,"name":entry.name,"path":display,"kind":entry.kind,
        "repository_id":repo.repository_id,"repository_path":entry.path,
        "container":entry.kind=="file","size":entry.size.to_string(),"file_id":entry.file_id,
        "version_id":entry.version_id,"selected_branch_id":context.as_ref().map(|context|context.branch_id.clone()).or_else(||engine.state().file_selections.get(&entry.file_id).cloned())});
    if entry.kind == "file" {
        response["read_only"] = context
            .as_ref()
            .is_some_and(|context| {
                context.inherited_from.is_none()
                    && engine
                        .state()
                        .branches
                        .get(&context.branch_id)
                        .is_some_and(|branch| branch.state != "active")
            })
            .into();
        response["node_id"] = context
            .as_ref()
            .and_then(|context| context.node_id.clone())
            .unwrap_or_else(|| entry.version_id.clone())
            .into();
        response.as_object_mut().unwrap().extend(
            content_wire(
                engine,
                Some(&entry.version_id),
                super::wire::current_entry_source_at_index(
                    engine,
                    &repo.main_branch_id,
                    index,
                    context.as_deref(),
                ),
            )
            .as_object()
            .unwrap()
            .clone(),
        );
    }
    Ok(response)
}
fn directory(name: &str, display: &str, id: &str) -> Value {
    json!({"entry_id":id,"name":name,"path":display,"kind":"directory","size":"0","file_id":"","version_id":""})
}

/// Read frozen branch scopes without changing the shared SMB/default selection.
/// A historical branch may retain a directory that is absent from the current
/// projection, so scope discovery must use stable directory identities.
fn scoped_directory_branches<'a>(
    engine: &'a Engine,
    repo: &str,
    requested_path: Option<&str>,
) -> Result<Vec<&'a storage_contract::DirectoryBranch>, ApiError> {
    let repository = repository(engine, repo)?;
    if let Some(path) = requested_path.filter(|path| !path.is_empty()) {
        name_policy::validate_lookup_path(path).map_err(ApiError::invalid)?;
    }
    let requested_key = requested_path.map(name_policy::path_key);
    let requested_directory_id = requested_key.as_ref().and_then(|key| {
        if key.is_empty() {
            return Some(repo.to_owned());
        }
        let namespace = engine.state().branches.get(&repository.main_branch_id)?;
        engine
            .state()
            .commits
            .get(&namespace.head_commit_id)?
            .entries
            .iter()
            .find(|entry| entry.kind == "directory" && entry.lookup_key == *key)
            .map(|entry| entry.entry_id.clone())
    });
    let mut scopes = engine
        .state()
        .directory_branches
        .values()
        .filter(|branch| {
            branch.repository_id == repo
                && requested_key.as_ref().is_none_or(|key| {
                    requested_directory_id.as_ref().map_or_else(
                        || name_policy::path_key(&branch.path) == *key,
                        |directory| &branch.directory_id == directory,
                    )
                })
        })
        .collect::<Vec<_>>();
    scopes.sort_by_key(|branch| {
        (
            Reverse(branch.sequence),
            branch.directory_branch_id.as_str(),
        )
    });
    Ok(scopes)
}

pub(super) fn directory_branch_scopes(
    engine: &Engine,
    repo: &str,
    requested_path: Option<&str>,
) -> Result<Value, ApiError> {
    let scopes = scoped_directory_branches(engine, repo, requested_path)?;
    let branches = scopes
        .into_iter()
        .map(|branch| {
            let mut value = encode(branch)?;
            value["current"] = (engine
                .state()
                .directory_selections
                .get(&branch.directory_id)
                == Some(&branch.directory_branch_id))
            .into();
            value["file_count"] = branch
                .entries
                .iter()
                .filter(|entry| entry.kind == "file")
                .count()
                .to_string()
                .into();
            Ok(value)
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(json!({"repository_id":repo,"directory_branches":branches}))
}

pub(super) fn directory_branch_summaries(
    engine: &Engine,
    repo: &str,
    path: Option<&str>,
) -> Result<Value, ApiError> {
    let branches = scoped_directory_branches(engine, repo, path)?
        .into_iter()
        .map(|branch| {
            listing::branch_summary(
                branch,
                engine
                    .state()
                    .directory_selections
                    .get(&branch.directory_id)
                    == Some(&branch.directory_branch_id),
            )
        })
        .collect::<Vec<_>>();
    Ok(json!({"repository_id":repo,"directory_branches":branches}))
}

fn root_has_virtual_directories(engine: &Engine, slot: &str) -> bool {
    engine
        .state()
        .repositories
        .values()
        .any(|repo| repo.primary_slot_id == slot && !repo.file_manager_root)
}

fn file_history(engine: &Engine, input: &Value) -> Result<Value, ApiError> {
    let file = field(input, "file_id")?;
    let repo = file_repo(engine, file)?;
    if let Some(query) = input.get("query") {
        keys(query, &["repository_id", "path"])?;
    }
    let state = engine.state();
    let requested_repo = query(input, "repository_id");
    let requested_path = query(input, "path");
    if requested_repo.is_some() != requested_path.is_some() {
        return Err(ApiError::invalid(
            "repository_id and path must be supplied together",
        ));
    }
    if let Some(path) = requested_path {
        name_policy::validate_lookup_path(path).map_err(ApiError::invalid)?;
    }
    let tree = engine.namespace_tree(&repo.main_branch_id).map_err(error)?;
    let entries = tree.entries();
    let current = if let Some(path) = requested_path {
        if requested_repo != Some(repo.repository_id.as_str()) {
            return Err(ApiError::new(
                404,
                "FILE_NOT_FOUND",
                "file is not present in this repository context",
            ));
        }
        Some(
            tree.get(path)
                .filter(|entry| entry.kind == "file" && entry.file_id == file)
                .cloned()
                .ok_or_else(|| {
                    ApiError::new(
                        404,
                        "FILE_NOT_FOUND",
                        "file is not present at this logical path",
                    )
                })?,
        )
    } else {
        state
            .file_locations
            .get(file)
            .and_then(|path| tree.get(path).filter(|entry| entry.file_id == file))
            .cloned()
            .or_else(|| entries.iter().find(|entry| entry.file_id == file).cloned())
    };
    let context = current
        .as_ref()
        .and_then(|entry| engine.file_context(&repo.repository_id, &entry.path));
    let current_node = context
        .as_ref()
        .and_then(|context| context.node_id.clone())
        .or_else(|| current.as_ref().map(|entry| entry.version_id.clone()));
    let selected = context
        .as_ref()
        .map(|context| context.branch_id.clone())
        .or_else(|| state.file_selections.get(file).cloned());
    let file_branches = state
        .branches
        .values()
        .filter(|branch| branch.file_id.as_deref() == Some(file))
        .collect::<Vec<_>>();
    let branches = file_branches
        .iter()
        .filter(|branch| branch.state != "migrated")
        .map(|branch| {
            let mut response = branch_wire(engine, branch)?;
            response["current"] = (selected.as_deref() == Some(branch.branch_id.as_str())).into();
            Ok(response)
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let history = engine
        .file_history(file)
        .map_err(error)?
        .into_iter()
        .filter(|version| version.history_tree_id == repo.history_tree_id)
        .collect::<Vec<_>>();
    let visible = history
        .iter()
        .map(|version| (version.version_id.as_str(), version))
        .collect::<BTreeMap<_, _>>();

    // Scan commit entries once. Re-scanning all commits for every version made
    // one file's history grow quadratically while holding the engine mutex.
    let mut first_commits = BTreeMap::<&str, &storage_contract::Commit>::new();
    for commit in state.commits.values() {
        for entry in &commit.entries {
            if visible.contains_key(entry.version_id.as_str()) {
                let first = first_commits.entry(&entry.version_id).or_insert(commit);
                if commit.sequence < first.sequence {
                    *first = commit;
                }
            }
        }
    }
    let mut heads = BTreeMap::new();
    for branch in file_branches {
        if let Some(commit) = state.commits.get(&branch.head_commit_id) {
            for entry in &commit.entries {
                // Keep the first branch in stable branch-id order, matching the
                // previous find() behavior if several branches share one head.
                if !branch.pending_file_versions.contains_key(&entry.file_id)
                    && !commit.pending_file_versions.contains_key(&entry.file_id)
                    && visible.contains_key(entry.version_id.as_str())
                {
                    heads
                        .entry(entry.version_id.as_str())
                        .or_insert(branch.branch_id.as_str());
                }
            }
        }
    }
    let mut versions = history.iter().map(|version| {
        // Only recorded earlier content edges in this visible file are exposed.
        // Response order and directory commit neighbors never invent ancestry.
        let mut seen = BTreeSet::new();
        let source_version_ids = version.source_version_ids.iter().filter(|id| {
            visible.get(id.as_str()).is_some_and(|parent| {
                parent.file_id == file && parent.version_sequence < version.version_sequence
            }) && seen.insert(id.as_str())
        }).collect::<Vec<_>>();
        let commit = first_commits.get(version.version_id.as_str()).copied();
        let download_name = current.as_ref().map(|entry| entry.name.as_str())
            .or_else(|| state.file_names.get(file).map(String::as_str))
            .unwrap_or(&version.creation_name);
        json!({
            "version_id":version.version_id,"file_id":file,"source_version_ids":source_version_ids,
            "branch_id":heads.get(version.version_id.as_str()),"created_at":version.created_at,
            "message":commit.map(|commit| commit.message.as_str()).unwrap_or(""),
            "size":version.logical_size.to_string(),"sequence":version.version_sequence.to_string(),
            "current":current.as_ref().is_some_and(|entry| entry.version_id == version.version_id),
            "content_state":state.content_states.get(&version.version_id).map(String::as_str).unwrap_or("LIVE"),
            "content_hash":version.content_hash,
            "note":state.version_notes.get(&version.version_id).map(String::as_str).unwrap_or(""),
            "download_name":download_name,
            "storage_name":version.storage_name
        })
    }).collect::<Vec<_>>();
    versions.reverse();
    let nodes = history_nodes::unified_history(
        engine,
        file,
        &repo.history_tree_id,
        &versions,
        &first_commits,
        current_node.as_deref(),
    )?;
    let mut response = json!({"file_id":file,"repository_id":repo.repository_id,"path":current.as_ref().map(|entry| &entry.path),
        "note":state.file_notes.get(file).map(String::as_str).unwrap_or(""),
        "node_id":current_node,"current_node_id":current_node,
        "selected_branch_id":selected,"versions":versions,"nodes":nodes,"branches":branches,
        "merges":state.file_branch_merges.values().filter(|merge|merge.file_id==file && merge.history_tree_id==repo.history_tree_id).collect::<Vec<_>>()});
    if let Some(entry) = &current {
        response.as_object_mut().unwrap().extend(
            content_wire(
                engine,
                Some(&entry.version_id),
                current_entry_source(engine, &repo.main_branch_id, entry),
            )
            .as_object()
            .unwrap()
            .clone(),
        );
    }
    Ok(response)
}

/// Capture admission before physical disk probes leave the engine lock.
pub(super) fn disk_access(engine: &Engine) -> BTreeMap<String, bool> {
    engine
        .config()
        .disks
        .iter()
        .map(|disk| (disk.slot_id.clone(), disk.level == 1))
        .collect()
}

/// Read handlers cannot obtain mutable Engine access or create lazy disk roots.
pub(super) fn read(
    engine: &Engine,
    action: crate::BackendAction,
    input: Value,
) -> Result<Value, ApiError> {
    match action {
        crate::BackendAction::FileManagerEntries => listing::entries(engine, &input),
        crate::BackendAction::FileManagerFileHistory => file_history(engine, &input),
        crate::BackendAction::FileManagerDownloadContext => {
            keys(&input, &["repository_id", "path", "version_id"])?;
            let repo = repository(engine, field(&input, "repository_id")?)?;
            let path = field(&input, "path")?;
            name_policy::validate_lookup_path(path).map_err(ApiError::invalid)?;
            let version = engine
                .state()
                .versions
                .get(field(&input, "version_id")?)
                .ok_or_else(|| ApiError::new(404, "VERSION_NOT_FOUND", "version missing"))?;
            if version.history_tree_id != repo.history_tree_id {
                return Err(ApiError::new(
                    404,
                    "FILE_NOT_FOUND",
                    "version is not in this repository context",
                ));
            }
            let entry = engine
                .namespace_tree(&repo.main_branch_id)
                .map_err(error)?
                .get(path)
                .filter(|entry| entry.kind == "file" && entry.file_id == version.file_id)
                .ok_or_else(|| {
                    ApiError::new(
                        404,
                        "FILE_NOT_FOUND",
                        "version file is not present at this logical path",
                    )
                })?;
            Ok(json!({"name":entry.name}))
        }
        crate::BackendAction::FileManagerHistory => activity::read(engine),
        crate::BackendAction::FileManagerBackupConfiguration => {
            let repo = field(&input, "repository_id")?;
            repository(engine, repo)?;
            engine
                .backup_configuration(repo, query(&input, "path").unwrap_or(""))
                .map_err(error)
        }
        _ => Err(ApiError::unsupported()),
    }
}

pub(super) fn call(
    engine: &mut Engine,
    identity: &Identity,
    action: crate::BackendAction,
    input: Value,
) -> Result<Value, ApiError> {
    match action {
        crate::BackendAction::FileManagerMerge
        | crate::BackendAction::FileManagerTags
        | crate::BackendAction::FileManagerConfigureBackups => {
            operations::call(engine, identity, action, &input)
        }
        crate::BackendAction::FileManagerDirectoryBranches
        | crate::BackendAction::FileManagerDirectorySelect => {
            keys(&input, &["slot_id", "path", "name", "operation_id"])?;
            let slot = field(&input, "slot_id")?;
            let display = path(&input)?;
            let name = field(&input, "name")?;
            storage_engine::validate_branch_name(name).map_err(error)?;
            let operation = operation(&input)?;
            storage_contract::validate_id(operation, "op").map_err(ApiError::invalid)?;
            if display.is_empty() && root_has_virtual_directories(engine, slot) {
                return Err(ApiError::new(
                    409,
                    "DIRECTORY_SCOPE_REQUIRED",
                    "select a specific folder before branching this disk root",
                ));
            }
            let access = if action == crate::BackendAction::FileManagerDirectoryBranches
                && display.is_empty()
            {
                DiskAccess::Create
            } else {
                DiskAccess::Write
            };
            if access == DiskAccess::Create {
                admit_root_operation(engine, slot, display, operation)?;
                if needs_disk_root(engine, slot, display) && name_policy::lookup_key(name) == "main"
                {
                    return Err(ApiError::conflict(
                        "NAME_COLLISION",
                        "directory baseline branch already uses this name",
                    ));
                }
            }
            let (repo, relative) = resolve(engine, identity, slot, display, access)?;
            let result = if action == crate::BackendAction::FileManagerDirectoryBranches {
                engine.create_directory_branches(
                    &repo.repository_id,
                    &relative,
                    name,
                    &identity.principal,
                    operation,
                )
            } else {
                engine.select_directory_branches(
                    &repo.repository_id,
                    &relative,
                    name,
                    &identity.principal,
                    operation,
                )
            }
            .map_err(error)?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerWriteSession => {
            keys(&input, &["slot_id", "path", "branch_id"])?;
            let display = path(&input)?.to_string();
            if display.is_empty() {
                return Err(ApiError::invalid("file path required"));
            }
            let requested_branch = input
                .get("branch_id")
                .map(|_| field(&input, "branch_id"))
                .transpose()?;
            if let Some(branch) = requested_branch {
                storage_contract::validate_id(branch, "br").map_err(ApiError::invalid)?;
            }
            // Selecting an existing branch cannot create a namespace as a side
            // effect when neither that branch nor its file exists.
            let access = if requested_branch.is_some() {
                DiskAccess::Write
            } else {
                DiskAccess::Create
            };
            let (repo, relative) = resolve(
                engine,
                identity,
                field(&input, "slot_id")?,
                &display,
                access,
            )?;
            if relative.is_empty() {
                return Err(ApiError::invalid("cannot upload over a disk directory"));
            }
            let tree = engine.namespace_tree(&repo.main_branch_id).map_err(error)?;
            let existing = tree.get(&relative);
            if let Some(context) =
                existing.and_then(|entry| engine.file_context(&repo.repository_id, &entry.path))
                && context.inherited_from.is_none()
                && engine
                    .state()
                    .branches
                    .get(&context.branch_id)
                    .is_some_and(|branch| branch.state != "active")
            {
                return Err(ApiError::conflict(
                    "BRANCH_NOT_ACTIVE",
                    "the selected file branch is read-only; select an active branch before writing",
                ));
            }
            if let Some(requested) = requested_branch {
                let selected = existing
                    .and_then(|entry| engine.file_context(&repo.repository_id, &entry.path))
                    .map(|context| context.branch_id.clone())
                    .or_else(|| {
                        existing.and_then(|entry| {
                            engine.state().file_selections.get(&entry.file_id).cloned()
                        })
                    });
                if selected.as_deref() != Some(requested) {
                    return Err(ApiError::conflict(
                        "FILE_BRANCH_CONFLICT",
                        "select this file branch before starting an upload",
                    ));
                }
            }
            // The namespace holds the directory composition's frozen version.
            // A shared file branch may already have a newer HEAD in another view.
            let workspace = engine
                .create_workspace(&repo.main_branch_id, &identity.principal)
                .map_err(error)?;
            let mut response = sanitized(workspace)?;
            response["relative_path"] = relative.into();
            response["display_path"] = display.into();
            Ok(response)
        }
        crate::BackendAction::FileManagerNamespace => {
            keys(&input, &["slot_id", "kind", "path", "operation_id"])?;
            let slot = field(&input, "slot_id")?;
            let kind = field(&input, "kind")?;
            if kind != "mkdir" {
                return Err(ApiError::invalid(
                    "only mkdir is supported; use identity-bound namespace-batch for existing entries",
                ));
            }
            let operation = operation(&input)?;
            storage_contract::validate_id(operation, "op").map_err(ApiError::invalid)?;
            let display = path(&input)?;
            if display.is_empty() {
                return Err(ApiError::invalid("disk namespace root cannot be changed"));
            }
            admit_root_operation(engine, slot, display, operation)?;
            // An empty root needs one entry per requested directory component.
            if needs_disk_root(engine, slot, display)
                && display.split('/').count() > engine.config().max_entries
            {
                return Err(ApiError::limit("directory entry limit exceeded"));
            }
            let (repo, relative) = resolve(engine, identity, slot, display, DiskAccess::Create)?;
            if relative.is_empty() {
                return Err(ApiError::invalid("disk namespace root cannot be changed"));
            }
            let result = engine
                .file_namespace(
                    &repo.repository_id,
                    NamespaceOperation {
                        kind: kind.into(),
                        path: relative,
                        destination: None,
                    },
                    &identity.principal,
                    operation,
                )
                .map_err(error)?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerNamespaceBatch => {
            keys(&input, &["slot_id", "kind", "items", "operation_id"])?;
            let slot = field(&input, "slot_id")?;
            let request: storage_contract::DirectoryNamespaceBatch =
                json_codec::from_value(json!({
                    "kind":input.get("kind"), "items":input.get("items")
                }))
                .map_err(ApiError::invalid)?;
            let result = Application::new(engine, identity).namespace_batch(
                application::NamespaceBatch {
                    slot_id: slot.into(),
                    request,
                    operation_id: operation(&input)?.into(),
                },
            )?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerFileMetadata
        | crate::BackendAction::FileManagerVersionMetadata => {
            let version = if action == crate::BackendAction::FileManagerVersionMetadata {
                Some(field(&input, "version_id")?)
            } else {
                None
            };
            keys(
                &input,
                if version.is_some() {
                    &["version_id", "note", "operation_id"]
                } else {
                    &["file_id", "note", "operation_id"]
                },
            )?;
            let file = if let Some(id) = version {
                engine
                    .state()
                    .versions
                    .get(id)
                    .ok_or_else(|| ApiError::new(404, "VERSION_NOT_FOUND", "version not found"))?
                    .file_id
                    .clone()
            } else {
                field(&input, "file_id")?.into()
            };
            file_repo(engine, &file)?;
            let note = input
                .get("note")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::invalid("note must be a string"))?;
            let result = engine
                .set_file_note(
                    &file,
                    version,
                    note,
                    &identity.principal,
                    operation(&input)?,
                )
                .map_err(error)?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerBranch => {
            keys(
                &input,
                &[
                    "file_id",
                    "source_version_id",
                    "source_node_id",
                    "name",
                    "operation_id",
                ],
            )?;
            let file = field(&input, "file_id")?;
            file_repo(engine, file)?;
            let result = if let Some(node) = input.get("source_node_id") {
                let node = node
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        ApiError::invalid("source_node_id must be a nonempty node ID")
                    })?;
                engine.create_file_branch_from_node(
                    file,
                    field(&input, "source_version_id")?,
                    node,
                    field(&input, "name")?,
                    &identity.principal,
                    operation(&input)?,
                )
            } else {
                engine.create_file_branch(
                    file,
                    field(&input, "source_version_id")?,
                    field(&input, "name")?,
                    &identity.principal,
                    operation(&input)?,
                )
            }
            .map_err(error)?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerNodeNote => {
            keys(&input, &["file_id", "node_id", "note", "operation_id"])?;
            let file = field(&input, "file_id")?;
            file_repo(engine, file)?;
            let note = input
                .get("note")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::invalid("note must be a string"))?;
            let result = engine
                .set_file_node_note(
                    file,
                    field(&input, "node_id")?,
                    note,
                    &identity.principal,
                    operation(&input)?,
                )
                .map_err(error)?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerSelect => {
            keys(
                &input,
                &[
                    "file_id",
                    "branch_id",
                    "operation_id",
                    "repository_id",
                    "path",
                ],
            )?;
            let file = field(&input, "file_id")?;
            let repo = file_repo(engine, file)?.repository_id.clone();
            let requested_repo = input.get("repository_id");
            let requested_path = input.get("path");
            if requested_repo.is_some() != requested_path.is_some() {
                return Err(ApiError::invalid(
                    "repository_id and path must be supplied together",
                ));
            }
            let result = if requested_repo.is_some() {
                if field(&input, "repository_id")? != repo {
                    return Err(ApiError::new(
                        404,
                        "FILE_NOT_FOUND",
                        "file is not in this repository",
                    ));
                }
                engine.select_file_branch_at(
                    &repo,
                    field(&input, "path")?,
                    file,
                    field(&input, "branch_id")?,
                    &identity.principal,
                    operation(&input)?,
                )
            } else {
                engine.select_file_branch(
                    file,
                    field(&input, "branch_id")?,
                    &identity.principal,
                    operation(&input)?,
                )
            }
            .map_err(error)?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerRenameBranch => {
            keys(&input, &["file_id", "branch_id", "name", "operation_id"])?;
            let file = field(&input, "file_id")?;
            file_repo(engine, file)?;
            let result = engine
                .rename_file_branch(
                    file,
                    field(&input, "branch_id")?,
                    field(&input, "name")?,
                    &identity.principal,
                    operation(&input)?,
                )
                .map_err(error)?;
            operation_wire(result)
        }
        _ => Err(ApiError::unsupported()),
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/file_manager/request_tests.rs"]
mod request_tests;

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/file_manager/history_query_tests.rs"]
mod history_query_tests;

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/file_manager/time_tests.rs"]
mod time_tests;

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/file_manager/listing_integration_tests.rs"]
mod listing_integration_tests;
