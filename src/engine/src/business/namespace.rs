//! Namespace changes preserve stable identity and path invariants.
use super::*;
use std::collections::BTreeMap;
mod draft;
mod query;
pub(crate) mod container_name;
pub(crate) use draft::NamespaceDraft;
pub use query::{CurrentObject, NamespaceTree};

/// Change only the display location of an existing entry. Stable entry/file/
/// version identities and content metadata survive moves, case-only renames,
/// and directory rebasing. Validate first so failure leaves the entry intact.
pub(crate) fn relocate_entry(entry: &mut Entry, path: name_policy::PortablePath) {
    entry.relocate(path);
}

/// Borrow strict descendants from a namespace already ordered by lookup key.
/// The empty key selects the repository root's full contents.
pub(crate) fn sorted_descendants<'a>(entries: &'a [Entry], key: &str) -> &'a [Entry] {
    if key.is_empty() {
        return entries;
    }
    let prefix = format!("{key}/");
    let start = entries.partition_point(|entry| entry.lookup_key() < prefix.as_str());
    let entries = &entries[start..];
    let end = entries.partition_point(|entry| entry.lookup_key().starts_with(&prefix));
    &entries[..end]
}

pub(crate) fn rebase_directory_entries(
    entries: &[Entry],
    from: &str,
    to: &str,
    own_root: fn() -> Error,
) -> Result<Vec<Entry>> {
    let depth = if from.is_empty() {
        0
    } else {
        from.split('/').count()
    };
    let target = (!to.is_empty())
        .then(|| name_policy::PortablePath::parse(to))
        .transpose()?;
    entries
        .iter()
        .map(|entry| {
            let path = match &target {
                Some(target) => entry.location().rebase(depth, target)?,
                None => entry
                    .location()
                    .strip_components(depth)
                    .ok_or_else(own_root)?,
            };
            let mut entry = entry.clone();
            entry.relocate(path);
            Ok(entry)
        })
        .collect()
}

pub(crate) fn sorted(entries: &mut [Entry]) {
    entries.sort_by(|a, b| a.lookup_key().as_bytes().cmp(b.lookup_key().as_bytes()));
}
/// Mutation buffers are deliberately unordered until `validate_entries`.
/// Retain/push and subtree renames preserve operation/deletion ordering; sorting
/// between operations would change that observable order. Published reads use
/// `NamespaceTree` instead, and indexed upload construction uses `NamespaceDraft`.
pub(crate) fn find_unordered<'a>(entries: &'a [Entry], path: &str) -> Option<&'a Entry> {
    let key = name_policy::path_key(path);
    find_unordered_key(entries, &key)
}
pub(crate) fn find_unordered_key<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries.iter().find(|entry| entry.lookup_key() == key)
}
pub(crate) fn validate_entries(entries: &mut [Entry], maximum: usize) -> Result<()> {
    if entries.len() > maximum {
        return Err(Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "directory entry budget exceeded",
        ));
    }

    // Parent checks share one index; path normalization belongs to the input type.
    let mut kinds = BTreeMap::new();
    for entry in entries.iter() {
        if kinds.insert(entry.lookup_key(), entry.kind).is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::NameCollision,
                "two entries have the same normalized lookup key",
            ));
        }
    }
    for entry in entries.iter() {
        if let Some((parent, _)) = entry.lookup_key().rsplit_once('/')
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
pub(crate) fn ensure_parents(
    entries: &mut Vec<Entry>,
    path: &name_policy::PortablePath,
) -> Result<()> {
    let mut parents = Vec::new();
    let mut parent = path.parent();
    while let Some(path) = parent {
        parent = path.parent();
        parents.push(path);
    }
    for path in parents.into_iter().rev() {
        if let Some(existing) = entries
            .iter()
            .find(|entry| entry.lookup_key() == path.key())
        {
            if existing.kind != NamespaceEntryKind::Directory {
                return Err(Error::new(
                    storage_contract::ErrorCode::ParentNotDirectory,
                    "parent is not a directory",
                ));
            }
        } else {
            entries.push(Entry {
                entry_id: new_id("entry")?,
                path,
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
        let key = op.key();
        if op.kind() == NamespaceOperationKind::Mkdir
            && find_unordered_key(base, key).is_none()
            && find_unordered_key(entries, key)
                .is_some_and(|e| e.kind == NamespaceEntryKind::Directory)
            && explicit_directories.insert(key)
        {
            continue;
        }
        deleted.extend(apply_namespace(entries, op)?);
        if op.kind() == NamespaceOperationKind::Mkdir {
            explicit_directories.insert(op.key());
        }
    }
    Ok(deleted)
}
/// Normal workspaces preserve every sealed upload through ordered renames.
/// Manifests do not record the order between uploads and deletion actions, so
/// a save cannot silently discard an upload. Track typed locations only; actual
/// namespace collision/shape validation remains in apply_workspace_namespace.
pub(crate) fn workspace_upload_locations(
    current: &[Entry],
    workspace: &Workspace,
) -> Result<BTreeMap<String, name_policy::PortablePath>> {
    workspace.files.values().map(|file| {
        let mut path = current.binary_search_by(|entry| entry.lookup_key().cmp(file.path.key()))
            .ok().map(|index| current[index].path.clone()).unwrap_or_else(|| file.path.clone());
        for operation in &workspace.namespace_operations {
            match operation {
                NamespaceOperation::Mkdir { .. } => {}
                NamespaceOperation::Delete { .. } if path.key() != operation.key() => {}
                NamespaceOperation::DeleteTree { .. }
                    if path.key() != operation.key() && !is_descendant(path.key(), operation.key()) => {}
                NamespaceOperation::Delete { .. } | NamespaceOperation::DeleteTree { .. } => {
                    return Err(Error::new(
                        storage_contract::ErrorCode::WorkspaceDependencyBusy,
                        "namespace edits would discard a sealed upload; staged bytes are preserved",
                    ));
                }
                NamespaceOperation::Rename { source, destination } => {
                    if path.key() == source.key() {
                        path = destination.clone();
                    } else if is_descendant(path.key(), source.key()) {
                        path = path.rebase(source.as_str().split('/').count(), destination)?;
                    }
                }
            }
        }
        Ok((file.item_id.clone(), path))
    }).collect()
}

pub(crate) fn validate_workspace_dependencies(workspace: &Workspace) -> Result<()> {
    let overlaps = |left: &str, right: &str| {
        left == right || is_descendant(left, right) || is_descendant(right, left)
    };
    for operation in &workspace.namespace_operations {
        if operation.kind() == NamespaceOperationKind::Mkdir {
            continue;
        }
        if workspace.files.values().any(|file| {
            overlaps(file.path.key(), operation.key())
                || operation
                    .destination()
                    .is_some_and(|path| overlaps(file.path.key(), path.key()))
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

/// Use the same nearest-owner rule for published lookups and mutable batches.
/// Each caller retains its existing index; the empty key denotes its root.
pub(crate) fn closest_directory_repository<'a>(
    mut key: &str,
    mut lookup: impl FnMut(&str) -> Option<&'a str>,
) -> Option<&'a str> {
    loop {
        if let Some(repository) = lookup(key) {
            return Some(repository);
        }
        if key.is_empty() {
            return None;
        }
        key = key.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

pub(crate) fn deleted_path(entry: &Entry) -> DeletedPath {
    DeletedPath {
        path: entry.path().to_owned(),
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
    let key = op.key();
    let mut deleted = Vec::new();
    match op {
        NamespaceOperation::Mkdir { path } => {
            if find_unordered_key(entries, key).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::NameCollision,
                    "directory target exists",
                ));
            }
            ensure_parents(entries, path)?;
            entries.push(Entry {
                entry_id: new_id("entry")?,
                path: path.clone(),
                kind: NamespaceEntryKind::Directory,
                file_id: String::new(),
                version_id: String::new(),
                size: 0,
            });
        }
        NamespaceOperation::Delete { .. } | NamespaceOperation::DeleteTree { .. } => {
            if find_unordered_key(entries, key).is_none() {
                return Err(Error::new(
                    storage_contract::ErrorCode::EntryNotFound,
                    "delete target missing",
                ));
            }
            if op.kind() == NamespaceOperationKind::Delete
                && entries.iter().any(|e| is_descendant(e.lookup_key(), key))
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::DirectoryNotEmpty,
                    "directory contains children",
                ));
            }
            deleted = entries
                .iter()
                .filter(|e| {
                    e.lookup_key() == key
                        || (op.kind() == NamespaceOperationKind::DeleteTree
                            && is_descendant(e.lookup_key(), key))
                })
                .map(deleted_path)
                .collect();
            entries.retain(|e| {
                e.lookup_key() != key
                    && !(op.kind() == NamespaceOperationKind::DeleteTree
                        && is_descendant(e.lookup_key(), key))
            });
        }
        NamespaceOperation::Rename {
            destination: target,
            ..
        } => {
            let to_key = target.key();
            let source = find_unordered_key(entries, key)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::EntryNotFound,
                        "rename source missing",
                    )
                })?;
            let renamed_file = (source.kind == NamespaceEntryKind::File)
                .then(|| source.file_id.parse::<storage_contract::FileId>())
                .transpose()?;
            let depth = source.path().split('/').count();
            if is_descendant(to_key, key) {
                return Err(Error::new(
                    storage_contract::ErrorCode::InvalidMove,
                    "cannot move a directory into itself",
                ));
            }
            if key != to_key && find_unordered_key(entries, target.key()).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::NameCollision,
                    "rename destination exists",
                ));
            }
            ensure_parents(entries, target)?;
            // Display spellings can differ in both case and UTF-8 length from
            // the normalized key (Straße / STRASSE). Select by key, then strip
            // display components rather than slicing by a different spelling.
            for entry in entries.iter_mut() {
                let path = if entry.lookup_key() == key {
                    target.clone()
                } else if is_descendant(entry.lookup_key(), key) {
                    entry.location().rebase(depth, target)?
                } else {
                    continue;
                };
                relocate_entry(entry, path);
            }
            if let Some(file) = renamed_file {
                let name = name_policy::Name::parse(target.file_name())?;
                container_name::apply_file_names(entries, &BTreeMap::from([(file, name)]))?;
            }
        }
    }
    Ok(deleted)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/namespace/tests.rs"]
mod tests;
