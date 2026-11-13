//! Pure leaf-name planning for current occurrences, never frozen history edits.
use crate::{Entry, NamespaceEntryKind};
use name_policy::{Name, PortablePath};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{EntryId, FileId};

#[derive(Debug, error_definition::Error)]
pub enum RenameError {
    #[error("current occurrence identity is invalid")]
    Identity(#[source] storage_contract::Error),
    #[error("current container name exceeds an occurrence's path budget")]
    Path(#[source] name_policy::Error),
    #[error("current container name collides at {0}")]
    Collision(PortablePath),
    #[error("one container cannot receive different names in the same mapping: {0}")]
    ConflictingNames(FileId),
}

impl RenameError {
    pub(crate) fn code(&self) -> storage_contract::ErrorCode {
        match self {
            Self::Identity(source) => source.code(),
            Self::Path(source) => storage_contract::ErrorCode::from(source),
            Self::Collision(_) => storage_contract::ErrorCode::NameCollision,
            Self::ConflictingNames(_) => storage_contract::ErrorCode::BatchMappingConflict,
        }
    }
}

/// Apply a complete plan only after every final destination has been checked.
pub(crate) fn apply_file_names(
    entries: &mut [Entry],
    names: &BTreeMap<FileId, Name>,
) -> crate::Result<Vec<FileNameChange>> {
    let changes = plan_file_names(entries, names).map_err(crate::Error::ContainerName)?;
    let paths: BTreeMap<_, _> = changes.iter()
        .map(|change| (change.before.key(), &change.after))
        .collect();
    for entry in entries.iter_mut() {
        if let Some(path) = paths.get(entry.lookup_key()) {
            entry.relocate((*path).clone());
        }
    }
    super::sorted(entries);
    Ok(changes)
}

/// Names come from the same admitted placement state used for physical access.
/// Historical entry paths never supply a fallback current name.
pub(crate) fn current_names(
    state: &crate::State,
    entries: &[Entry],
) -> crate::Result<BTreeMap<FileId, Name>> {
    entries.iter().filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| {
            Ok((
                entry.file_id.parse()?,
                crate::container_storage::current_container_name(state, &entry.file_id)?,
            ))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileNameChange {
    pub(crate) entry_id: EntryId,
    pub(crate) file_id: FileId,
    pub(crate) before: PortablePath,
    pub(crate) after: PortablePath,
}

/// Plan all names together so swaps are checked against their final destinations.
/// The caller supplies only an authorized current namespace and authoritative names.
/// Names absent from this scope contribute no changes; global source existence,
/// permission, generation and publication checks remain the caller's responsibility.
/// The returned identities and original paths must be bound again before publication.
pub(crate) fn plan_file_names(
    entries: &[Entry],
    names: &BTreeMap<FileId, Name>,
) -> std::result::Result<Vec<FileNameChange>, RenameError> {
    let mut changes = Vec::new();
    let mut destinations = BTreeSet::new();
    for entry in entries {
        let after = if entry.kind == NamespaceEntryKind::File {
            let file_id = entry.file_id.parse::<FileId>().map_err(RenameError::Identity)?;
            match names.get(&file_id) {
                Some(name) => {
                    let path = match entry.location().parent() {
                        Some(parent) => parent.join_name(name),
                        None => PortablePath::from_name(name),
                    }
                    .map_err(RenameError::Path)?;
                    if path != entry.path {
                        changes.push(FileNameChange {
                            entry_id: entry.entry_id.parse().map_err(RenameError::Identity)?,
                            file_id,
                            before: entry.path.clone(),
                            after: path.clone(),
                        });
                    }
                    path
                }
                None => entry.path.clone(),
            }
        } else {
            entry.path.clone()
        };
        if !destinations.insert(after.key().to_owned()) {
            return Err(RenameError::Collision(after));
        }
    }
    Ok(changes)
}

/// The requested occurrence may move parent; other occurrences only follow its name.
pub(crate) fn renamed_path(
    entry: &Entry,
    source: &name_policy::LookupPath,
    destination: &PortablePath,
    file: Option<FileId>,
) -> crate::Result<Option<PortablePath>> {
    if entry.lookup_key() == source.key() {
        return Ok(Some(destination.clone()));
    }
    if super::is_descendant(entry.lookup_key(), source.key()) {
        return Ok(Some(entry.location().rebase(
            source.as_str().split('/').count(), destination,
        )?));
    }
    if entry.kind == NamespaceEntryKind::File
        && file.is_some_and(|file| entry.file_id == file.to_string())
    {
        let name = Name::parse(destination.file_name())?;
        return Ok(Some(match entry.location().parent() {
            Some(parent) => parent.join_name(&name)?,
            None => PortablePath::from_name(&name)?,
        }));
    }
    Ok(None)
}

/// Recover each rename's exact source identity through chained operations.
/// Directory creation cannot change any existing file identity.
pub(crate) fn operation_files(
    entries: &[Entry],
    operations: &[crate::NamespaceOperation],
) -> crate::Result<Vec<Option<FileId>>> {
    let mut current = entries.to_vec();
    let mut files = Vec::with_capacity(operations.len());
    for operation in operations {
        match operation {
            crate::NamespaceOperation::Rename { source, destination } => {
                let file = current.iter().find(|entry| entry.lookup_key() == source.key())
                    .filter(|entry| entry.kind == NamespaceEntryKind::File)
                    .map(|entry| entry.file_id.parse()).transpose()?;
                files.push(file);
                for entry in &mut current {
                    if let Some(path) = renamed_path(entry, source, destination, file)? {
                        entry.relocate(path);
                    }
                }
            }
            crate::NamespaceOperation::Delete { .. } => {
                current.retain(|entry| entry.lookup_key() != operation.key());
                files.push(None);
            }
            crate::NamespaceOperation::DeleteTree { .. } => {
                current.retain(|entry| entry.lookup_key() != operation.key()
                    && !super::is_descendant(entry.lookup_key(), operation.key()));
                files.push(None);
            }
            crate::NamespaceOperation::Mkdir { .. } => files.push(None),
        }
    }
    Ok(files)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/namespace/container_name_tests.rs"]
mod tests;

/// Derive a runtime working source only after its frozen occurrence proof is
/// admitted. Contexts follow the exact old occurrence key, never another alias.
pub(crate) fn normalize_directory_branch(
    state: &crate::State,
    branch: &mut crate::DirectoryBranch,
) -> crate::Result<()> {
    let names = current_names(state, &branch.entries)?;
    let mut entries = branch.entries.clone();
    let changes = apply_file_names(&mut entries, &names)?;
    let mut contexts = branch.file_contexts.clone();
    let mut moved = Vec::with_capacity(changes.len());
    for change in changes {
        let context = contexts.remove(change.before.key())
            .ok_or_else(crate::persist::path_changes::invalid)?;
        moved.push((change.after.key().to_owned(), context));
    }
    for (key, context) in moved {
        if contexts.insert(key, context).is_some() {
            return Err(crate::persist::path_changes::invalid());
        }
    }
    branch.entries = entries;
    branch.file_contexts = contexts;
    Ok(())
}
