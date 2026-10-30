//! File browsing reuses bounded directory-first ordering and controlled relative
//! paths; published content remains managed by the storage transaction engine.
use super::*;
pub(super) mod activity;
mod backup_namespace;
pub(super) mod history_pages;
pub(super) mod listing;
mod pagination;
use std::collections::{BTreeMap, BTreeSet};

fn disk<'a>(engine: &'a Engine, slot: &str) -> Result<&'a storage_engine::DiskConfig, ApiError> {
    engine
        .config()
        .disks
        .iter()
        .find(|d| d.slot_id == slot)
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DiskNotFound,
                "disk not found",
            )
        })
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
    operation: storage_contract::OperationId,
) -> Result<bool, ApiError> {
    let needs_root = needs_disk_root(engine, slot, path);
    if needs_root
        && engine
            .published_receipt(operation)
            .map_err(crate::engine_backend::error)?
            .is_some()
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::IdempotencyKeyReused,
            "operation is already bound to another request",
        ));
    }
    Ok(needs_root)
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
    if !names.is_empty() {
        // Repository names are single components, so each candidate is one
        // point lookup against the disk root's paged namespace.
        let tree = engine.namespace_tree(&root.main_branch_id).map_err(error)?;
        for name in &names {
            if tree.get_key(name).map_err(error)?.is_some() {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::NameCollision,
                    "a named repository conflicts with a disk-root entry; use an explicit repository identity to rename the entry",
                ));
            }
        }
    }
    Ok(())
}

fn resolve_existing<'a>(
    engine: &Engine,
    slot: &str,
    value: &'a str,
    access: DiskAccess,
) -> Result<Option<(Repository, &'a str)>, ApiError> {
    let write = access != DiskAccess::Read;
    if write && disk(engine, slot)?.level != 1 {
        return Err(ApiError::new(
            ::http::StatusCode::FORBIDDEN,
            storage_contract::ErrorCode::DiskReadOnly,
            "backup disks are read only",
        ));
    }
    let relative = value;
    let existing = engine
        .state()
        .repositories
        .values()
        .find(|r| r.primary_slot_id == slot && r.file_manager_root)
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
            r.primary_slot_id == slot
                && !r.file_manager_root
                && name_policy::lookup_key(&r.name) == name_policy::lookup_key(first)
        })
        .cloned()
    {
        if !eligible(engine, slot, &repo) {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "directory is not stored on this disk",
            ));
        }
        return Ok(Some((
            repo,
            relative.split_once('/').map(|(_, p)| p).unwrap_or(""),
        )));
    }
    let Some(repo) = existing else {
        return Ok(None);
    };
    Ok(Some((repo, relative)))
}
fn empty_directory() -> ApiError {
    ApiError::new(
        ::http::StatusCode::NOT_FOUND,
        storage_contract::ErrorCode::DirectoryNotFound,
        "disk directory is empty",
    )
}

fn resolve_read<'a>(
    engine: &Engine,
    slot: &str,
    value: &'a str,
) -> Result<(Repository, &'a str), ApiError> {
    resolve_existing(engine, slot, value, DiskAccess::Read)?.ok_or_else(empty_directory)
}

fn resolve<'a>(
    engine: &mut Engine,
    identity: &Identity,
    slot: &str,
    value: &'a str,
    access: DiskAccess,
) -> Result<(Repository, &'a str), ApiError> {
    if let Some(resolved) = resolve_existing(engine, slot, value, access)? {
        return Ok(resolved);
    }
    if access != DiskAccess::Create {
        return Err(empty_directory());
    }

    let repo = engine
        .create_disk_root(
            slot,
            &identity.principal,
            storage_contract::OperationId::generate(),
        )
        .map_err(error)?;
    Ok((repo, value))
}

pub(super) fn file_repo<'a>(engine: &'a Engine, file: &str) -> Result<&'a Repository, ApiError> {
    let history = engine.file_history_owner(file).ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::FileNotFound,
            "file not found",
        )
    })?;
    repo_for_history(engine, history)
}
#[cfg(test)]
thread_local! { static LISTED_ENTRY_PROJECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// `entry` is this repository's own current-commit materialization, already
/// in hand at the caller; no listing or key lookup is repeated here.
fn listed_current_entry(
    view: &storage_engine::PublishedReadView,
    repo: &Repository,
    entry: &storage_contract::Entry,
    prefix: &str,
) -> crate::ListingEntry {
    let context = view.file_context_for_current_entry(&repo.repository_id, entry);
    let branch = &view.state().branches[&repo.main_branch_id];
    let commit = &view.state().commits[&branch.head_commit_id];
    let resolution =
        super::wire::PendingResolution::new(view.state(), &repo.main_branch_id, commit);
    let source = resolution.resolve(
        entry,
        entry,
        super::wire::PendingContexts {
            current: context.as_deref(),
            historical: None,
        },
        || view.pending_file_source(&repo.main_branch_id, &entry.file_id),
    );
    listed_entry(
        view.state(),
        repo,
        entry,
        prefix,
        context.as_deref(),
        source,
    )
}

/// Project the already resolved current entry without reading another namespace.
fn listed_entry(
    state: &storage_contract::State,
    repo: &Repository,
    entry: &storage_contract::Entry,
    prefix: &str,
    context: Option<&storage_contract::FileContext>,
    source: Option<&str>,
) -> crate::ListingEntry {
    use crate::{ListingEntry, ListingLocation, ListingVersion};
    #[cfg(test)]
    LISTED_ENTRY_PROJECTIONS.set(LISTED_ENTRY_PROJECTIONS.get() + 1);
    let display = if prefix.is_empty() {
        entry.path().to_owned()
    } else {
        format!("{prefix}/{}", entry.path)
    };
    let (version, node_id, read_only) = match entry.kind {
        storage_contract::NamespaceEntryKind::Directory => (
            ListingVersion::Directory(entry.version_id.clone()),
            None,
            None,
        ),
        storage_contract::NamespaceEntryKind::File => {
            let content = super::branch::capture_content(state, Some(&entry.version_id), source);
            let node = context
                .and_then(|context| context.node_id.clone())
                .unwrap_or_else(|| entry.version_id.clone());
            let read_only = context.is_some_and(|context| {
                context.inherited_from.is_none()
                    && state
                        .branches
                        .get(&context.branch_id)
                        .is_some_and(|branch| branch.state != storage_contract::BranchState::Active)
            });
            (
                ListingVersion::File {
                    content,
                    backup_version: None,
                },
                Some(node),
                Some(read_only),
            )
        }
    };
    ListingEntry {
        entry_id: entry.entry_id.clone(),
        name: entry.name().into(),
        path: display,
        kind: entry.kind,
        size: entry.size,
        file_id: entry.file_id.clone(),
        version,
        location: Some(ListingLocation {
            repository_id: repo.repository_id.clone(),
            repository_path: Some(entry.path().into()),
            container: entry.kind == storage_contract::NamespaceEntryKind::File,
            selected_branch_id: context
                .map(|context| context.branch_id.clone())
                .or_else(|| state.file_selections.get(&entry.file_id).cloned()),
        }),
        note: Some(
            state
                .file_notes
                .get(&entry.file_id)
                .cloned()
                .unwrap_or_default(),
        ),
        node_id,
        read_only,
        backup: false,
        historical_only: None,
    }
}
fn directory(name: &str, display: &str, id: &str) -> crate::ListingEntry {
    crate::ListingEntry {
        entry_id: id.into(),
        name: name.into(),
        path: display.into(),
        kind: storage_contract::NamespaceEntryKind::Directory,
        size: 0,
        file_id: String::new(),
        version: crate::ListingVersion::Directory(String::new()),
        location: None,
        note: None,
        node_id: None,
        read_only: None,
        backup: false,
        historical_only: None,
    }
}

fn root_has_virtual_directories(engine: &Engine, slot: &str) -> bool {
    engine
        .state()
        .repositories
        .values()
        .any(|repo| repo.primary_slot_id == slot && !repo.file_manager_root)
}

pub(super) fn mutate_directory(
    engine: &mut Engine,
    identity: &Identity,
    request: crate::MutateDiskDirectory,
) -> Result<storage_contract::OperationResult, ApiError> {
    let crate::MutateDiskDirectory {
        slot_id,
        path,
        name,
        operation_id,
        action,
    } = request;
    let display = path.as_ref().map_or("", name_policy::LookupPath::as_str);
    if display.is_empty() && root_has_virtual_directories(engine, &slot_id) {
        return Err(ApiError::new(
            ::http::StatusCode::CONFLICT,
            storage_contract::ErrorCode::DirectoryScopeRequired,
            "select a specific folder before branching this disk root",
        ));
    }
    let access = match action {
        crate::DiskDirectoryAction::Create if path.is_none() => DiskAccess::Create,
        crate::DiskDirectoryAction::Create | crate::DiskDirectoryAction::Select => {
            DiskAccess::Write
        }
    };
    if access == DiskAccess::Create
        && admit_root_operation(engine, &slot_id, display, operation_id)?
        && name.key() == "main"
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::NameCollision,
            "directory baseline branch already uses this name",
        ));
    }
    let (repo, _) = resolve(engine, identity, &slot_id, display, access)?;
    let relative = if repo.file_manager_root {
        path
    } else {
        path.expect("invariant: named repository resolution matched a path component")
            .strip_components(1)
    };
    match action {
        crate::DiskDirectoryAction::Create => engine.create_directory_branches(
            &repo.repository_id,
            relative.as_ref(),
            &name,
            &identity.principal,
            operation_id,
        ),
        crate::DiskDirectoryAction::Select => engine.select_directory_branches(
            &repo.repository_id,
            relative.as_ref(),
            &name,
            &identity.principal,
            operation_id,
        ),
    }
    .map_err(error)
}

pub(super) fn mkdir(
    engine: &mut Engine,
    identity: &Identity,
    command: crate::CreateDiskDirectory,
) -> Result<storage_contract::OperationResult, ApiError> {
    let crate::CreateDiskDirectory {
        slot_id,
        path,
        operation_id,
    } = command;
    let needs_root = admit_root_operation(engine, &slot_id, path.as_str(), operation_id)?;
    if needs_root && path.as_str().split('/').count() > engine.config().max_entries {
        return Err(ApiError::limit("directory entry limit exceeded"));
    }
    let (repo, _) = resolve(
        engine,
        identity,
        &slot_id,
        path.as_str(),
        DiskAccess::Create,
    )?;
    let path = if repo.file_manager_root {
        path
    } else {
        path.strip_components(1)
            .ok_or_else(|| ApiError::invalid("disk namespace root cannot be changed"))?
    };
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::Mkdir { path },
            &identity.principal,
            operation_id,
        )
        .map_err(error)
}

pub(super) fn write_session(
    engine: &mut Engine,
    identity: &Identity,
    command: crate::CreateDiskWriteSession,
) -> Result<crate::DiskWriteSession, ApiError> {
    use std::borrow::Cow;
    let crate::CreateDiskWriteSession {
        slot_id,
        path,
        branch_id,
    } = command;
    let access = if branch_id.is_some() {
        DiskAccess::Write
    } else {
        DiskAccess::Create
    };
    let (repo, _) = resolve(engine, identity, &slot_id, path.as_str(), access)?;
    let relative = if repo.file_manager_root {
        Cow::Borrowed(&path)
    } else {
        Cow::Owned(
            path.strip_components(1)
                .ok_or_else(|| ApiError::invalid("cannot upload over a disk directory"))?,
        )
    };
    let tree = engine.namespace_tree(&repo.main_branch_id).map_err(error)?;
    let existing = tree.get_key(relative.key()).map_err(error)?;
    let context = existing
        .as_ref()
        .and_then(|entry| engine.file_context_for_current_entry(&repo.repository_id, entry));
    if let Some(context) = context.as_ref()
        && context.inherited_from.is_none()
        && engine
            .state()
            .branches
            .get(&context.branch_id)
            .is_some_and(|branch| branch.state != storage_contract::BranchState::Active)
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::BranchNotActive,
            "the selected file branch is read-only; select an active branch before writing",
        ));
    }
    if let Some(requested) = branch_id {
        let selected = context
            .as_ref()
            .map(|context| context.branch_id.as_str())
            .or_else(|| {
                existing.as_ref().and_then(|entry| {
                    engine
                        .state()
                        .file_selections
                        .get(&entry.file_id)
                        .map(String::as_str)
                })
            });
        if selected != Some(requested.as_str()) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::FileBranchConflict,
                "select this file branch before starting an upload",
            ));
        }
    }
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &identity.principal)
        .map_err(error)?;
    Ok(crate::DiskWriteSession {
        workspace,
        relative_path: relative.as_str().to_owned(),
        display_path: path.into_string(),
    })
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
