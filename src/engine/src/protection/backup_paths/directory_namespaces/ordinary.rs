//! Metadata for the existing backup listing, borrowed from actual published
//! ordinary occurrences. No body reader or creation authority is introduced.
use crate::engine::fresh_layout::namespace::PublishedBackupSourcePath;
use crate::engine::fresh_layout::{FreshLayoutError, containers, directories};
use crate::protection::backup_paths::directory_namespaces::PublishedBackupDirectory;
use crate::protection::backup_paths::namespace;
use crate::{Engine, Entry, Error, NamespaceEntryKind, Result};
use name_policy::{PortablePath, SlotId};
use std::collections::BTreeMap;
use storage_contract::{DiskId, EntryId, FileId, RepositoryId};

/// The exact namespace occurrence behind one already-published replica.
/// Consumers retain this only as metadata under their original listing fence.
#[derive(Clone, Debug)]
pub struct PublishedBackupFilePath {
    repository: RepositoryId,
    entry: EntryId,
    source: PortablePath,
    display: PortablePath,
}

impl PublishedBackupFilePath {
    pub fn repository(&self) -> RepositoryId {
        self.repository
    }

    pub fn entry(&self) -> EntryId {
        self.entry
    }

    pub fn source(&self) -> &PortablePath {
        &self.source
    }

    pub fn display(&self) -> &PortablePath {
        &self.display
    }
}

fn limit(engine: &Engine, count: usize) -> Result<()> {
    if count >= engine.config().max_entries {
        return Err(Error::DirectoryLayout(Box::new(
            crate::DirectoryLayoutError::ResourceLimit,
        )));
    }
    Ok(())
}

fn entry(
    engine: &Engine,
    repository: RepositoryId,
    id: EntryId,
    kind: NamespaceEntryKind,
) -> Result<&Entry> {
    let current = namespace(engine.state(), &repository.to_string())?;
    current
        .entries
        .iter()
        .find(|entry| entry.entry_id == id.to_string() && entry.kind == kind)
        .ok_or_else(|| FreshLayoutError::NamespaceSource.into())
}

fn path(source: &PublishedBackupSourcePath<'_>, entry: &Entry) -> Result<PortablePath> {
    let prefix = source
        .destination()
        .relative_prefix()
        .ok_or(FreshLayoutError::NamespaceSource)?;
    Ok(PortablePath::parse(&format!("{prefix}/{}", entry.path()))?)
}

impl Engine {
    /// Supplies native display geometry to the existing backup listing only.
    /// Current/custom paths keep their original representation and consumers.
    pub fn published_backup_file_paths(
        &self,
        slot: &SlotId,
    ) -> Result<BTreeMap<FileId, PublishedBackupFilePath>> {
        let current = self.admitted_current_namespaces()?;
        let disk = self.disk(slot.as_str())?;
        if current.is_some() || disk.level == 1 {
            return Ok(BTreeMap::new());
        }
        let target = disk.disk_id.parse::<DiskId>()?;
        let mut paths = BTreeMap::new();
        for (file, record) in self.state().file_containers.iter().filter(|(_, record)| {
            record.owner_disk != target && record.replicas.contains_key(&target)
        }) {
            // Explicit custom bindings remain owned by the existing path map.
            if self
                .state()
                .file_backup_paths
                .get(&file.to_string())
                .is_some_and(|paths| paths.contains_key(slot.as_str()))
            {
                continue;
            }
            limit(self, paths.len())?;
            let source = containers::published_backup_source(
                self.config(),
                &self.disk_resources,
                self.state(),
                record,
                target,
            )?;
            let namespace = &record.namespace;
            let actual = entry(
                self,
                namespace.repository,
                namespace.entry,
                NamespaceEntryKind::File,
            )?;
            if actual.file_id != file.to_string() {
                return Err(FreshLayoutError::NamespaceSource.into());
            }
            paths.insert(
                *file,
                PublishedBackupFilePath {
                    repository: namespace.repository,
                    entry: namespace.entry,
                    source: actual.path.clone(),
                    display: path(&source, actual)?,
                },
            );
        }
        Ok(paths)
    }
}

pub(super) fn published_directories(
    engine: &Engine,
    target: DiskId,
) -> Result<Vec<PublishedBackupDirectory>> {
    let mut rows = Vec::new();
    for record in engine
        .state()
        .namespace_directories
        .values()
        .filter(|record| record.owner_disk != target && record.replicas.contains_key(&target))
    {
        limit(engine, rows.len())?;
        let source = directories::read_published_backup(
            engine.config(),
            &engine.disk_resources,
            engine.state(),
            record,
            target,
        )?;
        let actual = entry(
            engine,
            record.repository,
            record.entry,
            NamespaceEntryKind::Directory,
        )?;
        let repository = &engine.state().repositories[&record.repository.to_string()];
        rows.push(PublishedBackupDirectory {
            repository_id: repository.repository_id.clone(),
            history_tree_id: repository.history_tree_id.clone(),
            directory_id: record.entry.to_string(),
            path: path(&source, actual)?,
            operation_id: record.replicas[&target].created_by,
        });
    }
    rows.sort_by(|left, right| left.path.key().cmp(right.path.key()));
    Ok(rows)
}
