//! Namespace changes preserve stable identity and path invariants.
use super::*;
use std::collections::BTreeMap;
mod draft;
mod query;
pub(crate) use draft::NamespaceDraft;
pub use query::NamespaceTree;

/// Change only the display location of an existing entry. Stable entry/file/
/// version identities and content metadata survive moves, case-only renames,
/// and directory rebasing. Validate first so failure leaves the entry intact.
pub(crate) fn relocate_entry(entry: &mut Entry, path: String) -> Result<()> {
    name_policy::validate_path(&path)?;
    let name = path.rsplit('/').next().unwrap_or("").to_owned();
    let lookup_key = name_policy::path_key(&path);
    entry.path = path;
    entry.name = name;
    entry.lookup_key = lookup_key;
    Ok(())
}

pub(crate) fn sorted(entries: &mut [Entry]) {
    entries.sort_by(|a, b| a.lookup_key.as_bytes().cmp(b.lookup_key.as_bytes()));
}
/// Mutation buffers are deliberately unordered until `validate_entries`.
/// Retain/push and subtree renames preserve operation/deletion ordering; sorting
/// between operations would change that observable order. Published reads use
/// `NamespaceTree` instead, and indexed upload construction uses `NamespaceDraft`.
pub(crate) fn find_unordered<'a>(entries: &'a [Entry], path: &str) -> Option<&'a Entry> {
    let key = name_policy::path_key(path);
    entries.iter().find(|e| e.lookup_key == key)
}
pub(crate) fn validate_entries(entries: &mut [Entry], maximum: usize) -> Result<()> {
    if entries.len() > maximum {
        return Err(Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "directory entry budget exceeded",
        ));
    }

    for e in entries.iter_mut() {
        relocate_entry(e, e.path.clone())?;
    }
    // Build once after normalization; parent validation must not scan the
    // complete directory for every entry (up to max_entries per commit).
    let mut kinds = BTreeMap::new();
    for entry in entries.iter() {
        if kinds
            .insert(entry.lookup_key.as_str(), entry.kind)
            .is_some()
        {
            return Err(Error::new(
                storage_contract::ErrorCode::NameCollision,
                "two entries have the same normalized lookup key",
            ));
        }
    }
    for entry in entries.iter() {
        if let Some((parent, _)) = entry.lookup_key.rsplit_once('/')
            && kinds.get(parent).copied() != Some(NamespaceEntryKind::Directory)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ParentNotDirectory,
                "file parent is missing or is not a directory",
            ));
        }
    }
    sorted(entries);
    Ok(())
}
pub(crate) fn ensure_parents(entries: &mut Vec<Entry>, path: &str) -> Result<()> {
    let parts = path.split('/').collect::<Vec<_>>();
    let mut p = String::new();
    for c in parts.iter().take(parts.len().saturating_sub(1)) {
        if !p.is_empty() {
            p.push('/');
        }
        p.push_str(c);
        if let Some(existing) = find_unordered(entries, &p) {
            if existing.kind != NamespaceEntryKind::Directory {
                return Err(Error::new(
                    storage_contract::ErrorCode::ParentNotDirectory,
                    "parent is not a directory",
                ));
            }
        } else {
            entries.push(Entry {
                entry_id: new_id("entry"),
                path: p.clone(),
                name: (*c).into(),
                lookup_key: name_policy::path_key(&p),
                kind: NamespaceEntryKind::Directory,
                file_id: String::new(),
                version_id: String::new(),
                size: 0,
            });
        }
    }
    Ok(())
}
pub(crate) fn apply_workspace_namespace(
    entries: &mut Vec<Entry>,
    base: &[Entry],
    operations: &[NamespaceOperation],
) -> Result<Vec<DeletedPath>> {
    let mut deleted = Vec::new();
    let mut explicit_directories = std::collections::BTreeSet::new();
    for op in operations {
        // Uploaded children may have materialized their missing parents. An
        // explicit mkdir for that new parent belongs to the same workspace.
        let key = name_policy::path_key(&op.path);
        if op.kind == NamespaceOperationKind::Mkdir
            && find_unordered(base, &op.path).is_none()
            && find_unordered(entries, &op.path)
                .is_some_and(|e| e.kind == NamespaceEntryKind::Directory)
            && explicit_directories.insert(key)
        {
            continue;
        }
        deleted.extend(apply_namespace(entries, op)?);
        if op.kind == NamespaceOperationKind::Mkdir {
            explicit_directories.insert(name_policy::path_key(&op.path));
        }
    }
    Ok(deleted)
}
pub(crate) fn validate_workspace_dependencies(workspace: &Workspace) -> Result<()> {
    let overlaps = |left: &str, right: &str| {
        let left = name_policy::path_key(left);
        let right = name_policy::path_key(right);
        left == right || is_descendant(&left, &right) || is_descendant(&right, &left)
    };
    for operation in &workspace.namespace_operations {
        if operation.kind == NamespaceOperationKind::Mkdir {
            continue;
        }
        if workspace.files.values().any(|file| {
            overlaps(&file.path, &operation.path)
                || operation
                    .destination
                    .as_ref()
                    .is_some_and(|path| overlaps(&file.path, path))
        }) {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceDependencyBusy,
                "overlapping upload and delete/rename require separate explicit saves; staged bytes are preserved",
            ));
        }
    }
    Ok(())
}

/// Compare already-normalized keys at a complete path-component boundary.
pub(crate) fn is_descendant(key: &str, ancestor: &str) -> bool {
    key.strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

pub(crate) fn deleted_path(entry: &Entry) -> DeletedPath {
    DeletedPath {
        path: entry.path.clone(),
        kind: entry.kind,
    }
}

pub(crate) fn deletion_record(
    paths: Vec<DeletedPath>,
    timestamp: u64,
) -> Option<NamespaceDeletions> {
    (!paths.is_empty()).then_some(NamespaceDeletions { timestamp, paths })
}

pub(crate) fn apply_namespace(
    entries: &mut Vec<Entry>,
    op: &NamespaceOperation,
) -> Result<Vec<DeletedPath>> {
    let key = name_policy::path_key(&op.path);
    let mut deleted = Vec::new();
    match op.kind {
        NamespaceOperationKind::Mkdir => {
            if find_unordered(entries, &op.path).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::NameCollision,
                    "directory target exists",
                ));
            }
            ensure_parents(entries, &op.path)?;
            entries.push(Entry {
                entry_id: new_id("entry"),
                path: op.path.clone(),
                name: op.path.rsplit('/').next().unwrap_or("").into(),
                lookup_key: key,
                kind: NamespaceEntryKind::Directory,
                file_id: String::new(),
                version_id: String::new(),
                size: 0,
            });
        }
        NamespaceOperationKind::Delete | NamespaceOperationKind::DeleteTree => {
            if find_unordered(entries, &op.path).is_none() {
                return Err(Error::new(
                    storage_contract::ErrorCode::EntryNotFound,
                    "delete target missing",
                ));
            }
            if op.kind == NamespaceOperationKind::Delete
                && entries.iter().any(|e| is_descendant(&e.lookup_key, &key))
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::DirectoryNotEmpty,
                    "directory contains children",
                ));
            }
            deleted = entries
                .iter()
                .filter(|e| {
                    e.lookup_key == key
                        || (op.kind == NamespaceOperationKind::DeleteTree
                            && is_descendant(&e.lookup_key, &key))
                })
                .map(deleted_path)
                .collect();
            entries.retain(|e| {
                e.lookup_key != key
                    && !(op.kind == NamespaceOperationKind::DeleteTree
                        && is_descendant(&e.lookup_key, &key))
            });
        }
        NamespaceOperationKind::Rename => {
            let target = op.destination.as_ref().ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::InvalidRequest,
                    "rename needs destination",
                )
            })?;
            let to_key = name_policy::path_key(target);
            let original = find_unordered(entries, &op.path).cloned().ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::EntryNotFound,
                    "rename source missing",
                )
            })?;
            if is_descendant(&to_key, &key) {
                return Err(Error::new(
                    storage_contract::ErrorCode::InvalidMove,
                    "cannot move a directory into itself",
                ));
            }
            if key != to_key && find_unordered(entries, target).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::NameCollision,
                    "rename destination exists",
                ));
            }
            ensure_parents(entries, target)?;
            // Display spellings can differ in both case and UTF-8 length from
            // the normalized key (Straße / STRASSE). Select by key, then strip
            // display components rather than slicing by a different spelling.
            let depth = original.path.split('/').count();
            for entry in entries.iter_mut() {
                let path = if entry.lookup_key == key {
                    target.clone()
                } else if is_descendant(&entry.lookup_key, &key) {
                    let suffix = entry
                        .path
                        .splitn(depth + 1, '/')
                        .nth(depth)
                        .ok_or_else(|| {
                            Error::new(
                                storage_contract::ErrorCode::StateInvalid,
                                "entry path and lookup depth differ",
                            )
                        })?;
                    format!("{target}/{suffix}")
                } else {
                    continue;
                };
                relocate_entry(entry, path)?;
            }
        }
    }
    Ok(deleted)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/namespace/tests.rs"]
mod tests;
