//! Existing-file sparse edits retain logical paths while physical bodies move.
pub(crate) mod namespace;
use data_serialization::{Deserialize, Serialize};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntryContent, SnapshotFile, SnapshotFileSource,
    SnapshotManifestError, SnapshotReference, ContainerDiskError, require_container_disk,
};
use name_policy::{LookupPath, PortablePath};
use crate::directory_worktree::projection::namespace::{Content, Entry};
use platform_io::fs_safety::EntryName;
use std::collections::BTreeMap;
use storage_contract::{BranchId, DiskId, FileId, FileNodeId, VersionId};
use storage_contract::working_tree::LocalEntityIdentity;

/// Complete bytes already admitted from this instance's held sparse directory.
/// A leaf or a hash alone is not authority to read or move this material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkingFile {
    pub(crate) file: SnapshotFile,
    pub(crate) leaf: EntryName,
    pub(crate) entity: LocalEntityIdentity,
}

/// A completed, explicitly identified write operation, never a content-match
/// guess. Hashes describe integrity only; they do not create or erase this fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileEdit {
    pub(crate) path: PortablePath,
    pub(crate) material: WorkingFile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerPresence {
    Present { disk: DiskId, entity: LocalEntityIdentity },
    /// Only the current admitted disk inventory can establish this state.
    /// Offline, revoked, corrupt or unreadable locations are errors, not absence.
    Absent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerPublication {
    Update {
        file: FileId,
        disk: DiskId,
        source_version: VersionId,
        source_branch: BranchId,
        source_node: FileNodeId,
        entity: LocalEntityIdentity,
    },
    SnapshotOnly { file: FileId },
    /// The exact version already has a verified file-publication result. Its
    /// body can still MOVE for a directory commit without another FILE node.
    Retained { file: SnapshotFile, source: SnapshotFileSource },
}

#[derive(Debug, PartialEq)]
pub(crate) enum ReadRoute<'a> {
    Directory,
    Original { file: SnapshotFile },
    Working { material: &'a WorkingFile },
}

#[derive(Debug, PartialEq)]
pub(crate) enum Transfer<'a> {
    Directory,
    CopyOriginal { file: SnapshotFile },
    MoveWorking {
        material: &'a WorkingFile,
        container: ContainerPublication,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FileResult {
    pub(crate) path: PortablePath,
    pub(crate) material: SnapshotFile,
    pub(crate) source: SnapshotFileSource,
}

#[derive(Debug, PartialEq)]
pub(crate) struct PublishedFileSource {
    pub(crate) path: PortablePath,
    pub(crate) source: SnapshotFileSource,
}
#[derive(Debug, PartialEq)]
pub(crate) struct PlannedEntry<'a> {
    pub(crate) path: &'a PortablePath,
    pub(crate) transfer: Transfer<'a>,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum ProjectionError {
    #[error("sparse edit does not identify one original snapshot file")]
    Source,
    #[error("sparse edit repeats a logical file occurrence")]
    Duplicate,
    #[error("sparse edit changes immutable version identity without allocating a new version")]
    Version,
    #[error("published directory node differs from the complete submitted logical view")]
    PublishedView,
    #[error("complete worktree material exceeds snapshot admission or conflicts")]
    Material(#[source] SnapshotManifestError),
    #[error("published file baseline is not bound to this sparse occurrence or version")]
    FileResult,
}

#[derive(Debug, PartialEq)]
pub(crate) enum PlanError<E> {
    Locate(E),
    ContainerDisk(ContainerDiskError),
}

pub(crate) struct SparseProjection<'a> {
    source: &'a AdmittedSnapshotManifest,
    entries: Vec<Entry>,
    edits: BTreeMap<usize, WorkingFile>,
    results: BTreeMap<usize, FileResult>,
}
impl<'a> SparseProjection<'a> {
    /// The complete original manifest supplies untouched files and empty dirs.
    /// Explicit namespace batches retain source indexes in this same projection;
    /// an omitted write never implicitly removes a logical entry.
    pub(crate) fn admit(
        source: &'a AdmittedSnapshotManifest,
        edits: Vec<FileEdit>,
    ) -> Result<Self, ProjectionError> {
        let mut by_index = BTreeMap::new();
        for edit in edits {
            let index = source.entries().binary_search_by(|entry| {
                entry.path.key().cmp(edit.path.key())
            }).map_err(|_| ProjectionError::Source)?;
            let entry = &source.entries()[index];
            let SnapshotEntryContent::File { material: original, .. } = entry.content else {
                return Err(ProjectionError::Source);
            };
            if entry.path != edit.path || original.file_id != edit.material.file.file_id {
                return Err(ProjectionError::Source);
            }
            if source.files().contains_key(&edit.material.file.version_id) {
                return Err(ProjectionError::Version);
            }
            if by_index.insert(index, edit.material).is_some() {
                return Err(ProjectionError::Duplicate);
            }
        }
        let replacements = by_index.iter().map(|(index, material)| {
            (source.entries()[*index].path.clone(), material.file)
        }).collect::<Vec<_>>();
        source.validate_material_replacements(&replacements).map_err(ProjectionError::Material)?;
        let entries = source.entries().iter().enumerate().map(|(index, entry)| Entry {
            path: entry.path.clone(),
            content: match entry.content {
                SnapshotEntryContent::Directory {} => Content::Directory,
                SnapshotEntryContent::File { material, source } => Content::File {
                    original: index, material, source,
                },
            },
        }).collect();
        Ok(Self { source, entries, edits: by_index, results: BTreeMap::new() })
    }

    /// Results come from the exact independently verified completion records,
    /// never from content matching or the container's current default branch.
    pub(crate) fn admit_with_results(
        source: &'a AdmittedSnapshotManifest,
        edits: Vec<FileEdit>,
        results: &[FileResult],
    ) -> Result<Self, ProjectionError> {
        let mut view = Self::admit(source, edits)?;
        for (position, result) in results.iter().enumerate() {
            if position > 0 && results[position - 1].path.key() >= result.path.key() {
                return Err(ProjectionError::FileResult);
            }
            let index = source.entries().binary_search_by(|entry| {
                entry.path.key().cmp(result.path.key())
            }).map_err(|_| ProjectionError::FileResult)?;
            let entry = &source.entries()[index];
            let SnapshotEntryContent::File { material, source: origin } = entry.content else {
                return Err(ProjectionError::FileResult);
            };
            let edited = view.edits.get(&index).ok_or(ProjectionError::FileResult)?;
            if entry.path != result.path || material.file_id != result.material.file_id
                || (edited.file.version_id == result.material.version_id
                    && edited.file != result.material)
                || (origin == SnapshotFileSource::SnapshotOnly
                    && result.source != SnapshotFileSource::SnapshotOnly)
            {
                return Err(ProjectionError::FileResult);
            }
            if let SnapshotFileSource::Container { node, .. } = result.source {
                if node != FileNodeId::Version(result.material.version_id) {
                    return Err(ProjectionError::FileResult);
                }
            }
            view.results.insert(index, result.clone());
        }
        Ok(view)
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &PortablePath> {
        self.entries.iter().map(|entry| &entry.path)
    }

    pub(crate) fn route(&self, path: &LookupPath) -> Option<ReadRoute<'_>> {
        let index = self.entries.binary_search_by(|entry| entry.path.key().cmp(path.key())).ok()?;
        Some(match self.entries[index].content {
            Content::Directory => ReadRoute::Directory,
            Content::File { original, material: file, .. } => match self.edits.get(&original) {
                Some(material) => ReadRoute::Working { material },
                None => ReadRoute::Original { file },
            },
        })
    }

    /// Resolve stable container identity only for explicit modification facts. The
    /// owner performs these observations at commit admission on the repository's
    /// stable disk, including common-data and deleted-data. It must never search
    /// another disk or convert an offline/unreadable disk into proven absence.
    pub(crate) fn plan<E>(
        &self,
        repository_disk: DiskId,
        mut locate: impl FnMut(DiskId, FileId) -> Result<ContainerPresence, E>,
    ) -> Result<Vec<PlannedEntry<'_>>, PlanError<E>> {
        let mut result = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let transfer = match entry.content {
                Content::Directory => Transfer::Directory,
                Content::File { original, material, source } => {
                    self.file_transfer(original, material, source, repository_disk, &mut locate)?
                }
            };
            result.push(PlannedEntry { path: &entry.path, transfer });
        }
        Ok(result)
    }

    fn file_transfer<E>(
        &self,
        index: usize,
        file: SnapshotFile,
        source: SnapshotFileSource,
        repository_disk: DiskId,
        locate: &mut impl FnMut(DiskId, FileId) -> Result<ContainerPresence, E>,
    ) -> Result<Transfer<'_>, PlanError<E>> {
        Ok(match self.edits.get(&index) {
            Some(material) => {
                let baseline = self.results.get(&index);
                let (file, source) = match baseline {
                    Some(saved) => (saved.material, saved.source),
                    None => (file, source),
                };
                let container = if baseline.is_some()
                    && file.version_id == material.file.version_id
                {
                    ContainerPublication::Retained { file, source }
                } else {
                    match source {
                        SnapshotFileSource::SnapshotOnly => {
                            ContainerPublication::SnapshotOnly { file: file.file_id }
                        }
                        SnapshotFileSource::Container { branch, node } => {
                            match locate(repository_disk, file.file_id)
                                .map_err(PlanError::Locate)?
                            {
                                ContainerPresence::Present { disk, entity } => {
                                    require_container_disk(
                                        repository_disk, file.file_id, disk,
                                    ).map_err(PlanError::ContainerDisk)?;
                                    ContainerPublication::Update {
                                        file: file.file_id,
                                        disk,
                                        source_version: file.version_id,
                                        source_branch: branch,
                                        source_node: node,
                                        entity,
                                    }
                                }
                                ContainerPresence::Absent => {
                                    ContainerPublication::SnapshotOnly { file: file.file_id }
                                }
                            }
                        }
                    }
                };
                Transfer::MoveWorking { material, container }
            }
            None => Transfer::CopyOriginal { file },
        })
    }

    /// After the permanent move/copy decision has completed, publish the new
    /// node's route while keeping every logical path. Never refill the worktree.
    pub(crate) fn published_view(
        &self,
        node: &AdmittedSnapshotManifest,
        published_sources: &[PublishedFileSource],
    ) -> Result<SnapshotReference, ProjectionError> {
        let visible_edits = self.entries.iter().filter(|entry| match entry.content {
            Content::Directory => false,
            Content::File { original, .. } => self.edits.contains_key(&original),
        }).count();
        if published_sources.len() != visible_edits
            || node.repository_id() != self.source.repository_id()
            || node.node_id() == self.source.node_id()
            || node.entries().len() != self.entries.len()
        {
            return Err(ProjectionError::PublishedView);
        }
        for (entry, published) in self.entries.iter().zip(node.entries()) {
            if entry.path != published.path
                || self.final_content(entry, published_sources)? != published.content
            {
                return Err(ProjectionError::PublishedView);
            }
        }
        Ok(node.reference())
    }

    fn final_content(
        &self,
        entry: &Entry,
        published_sources: &[PublishedFileSource],
    ) -> Result<SnapshotEntryContent, ProjectionError> {
        let (index, file, source) = match entry.content {
            Content::Directory => return Ok(SnapshotEntryContent::Directory {}),
            Content::File { original, material, source } => (original, material, source),
        };
        Ok(match self.edits.get(&index) {
            Some(material) => {
                let mut selected = published_sources.iter()
                    .filter(|item| item.path == entry.path);
                let published_source = selected.next()
                    .ok_or(ProjectionError::PublishedView)?.source;
                if selected.next().is_some() {
                    return Err(ProjectionError::PublishedView);
                }
                let source = match self.results.get(&index) {
                    Some(saved) => saved.source,
                    None => source,
                };
                if source == SnapshotFileSource::SnapshotOnly
                    && published_source != SnapshotFileSource::SnapshotOnly
                {
                    return Err(ProjectionError::PublishedView);
                }
                SnapshotEntryContent::File { material: material.file, source: published_source }
            }
            None => SnapshotEntryContent::File { material: file, source },
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/projection.rs"]
mod tests;
