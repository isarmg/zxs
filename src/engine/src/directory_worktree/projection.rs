//! Existing-file sparse edits retain logical paths while physical bodies move.
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile, SnapshotFileSource,
    SnapshotReference,
};
use name_policy::{LookupPath, PortablePath};
use platform_io::fs_safety::EntryName;
use std::collections::BTreeMap;
use storage_contract::{BranchId, FileId, FileNodeId, VersionId};
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
    Present { entity: LocalEntityIdentity },
    /// Only the current admitted disk inventory can establish this state.
    /// Offline, revoked, corrupt or unreadable locations are errors, not absence.
    Absent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerPublication {
    Update {
        file: FileId,
        source_version: VersionId,
        source_branch: BranchId,
        source_node: FileNodeId,
        entity: LocalEntityIdentity,
    },
    SnapshotOnly { file: FileId },
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
}

pub(crate) struct SparseProjection<'a> {
    source: &'a AdmittedSnapshotManifest,
    edits: BTreeMap<usize, WorkingFile>,
}
impl<'a> SparseProjection<'a> {
    /// The complete original manifest supplies untouched files and empty dirs.
    /// This slice handles replacements only; namespace edits require their own
    /// admitted complete-view operation, never an implicit dropped entry.
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
            if original.version_id == edit.material.file.version_id {
                return Err(ProjectionError::Version);
            }
            if by_index.insert(index, edit.material).is_some() {
                return Err(ProjectionError::Duplicate);
            }
        }
        Ok(Self { source, edits: by_index })
    }

    pub(crate) fn route(&self, path: &LookupPath) -> Option<ReadRoute<'_>> {
        let index = self.source.entries().binary_search_by(|entry| {
            entry.path.key().cmp(path.key())
        }).ok()?;
        Some(match self.source.entries()[index].content {
            SnapshotEntryContent::Directory {} => ReadRoute::Directory,
            SnapshotEntryContent::File { material: file, .. } => match self.edits.get(&index) {
                Some(material) => ReadRoute::Working { material },
                None => ReadRoute::Original { file },
            },
        })
    }

    /// Resolve stable container identity only for explicit modification facts. The
    /// owner performs these observations at commit admission; arbitrary path
    /// NotFound or disk failure must not be converted into ContainerPresence::Absent.
    pub(crate) fn plan<E>(
        &self,
        mut locate: impl FnMut(FileId) -> Result<ContainerPresence, E>,
    ) -> Result<Vec<PlannedEntry<'_>>, E> {
        let mut result = Vec::with_capacity(self.source.entries().len());
        for (index, entry) in self.source.entries().iter().enumerate() {
            let transfer = match entry.content {
                SnapshotEntryContent::Directory {} => Transfer::Directory,
                SnapshotEntryContent::File {
                    material: file, source,
                } => match self.edits.get(&index) {
                    Some(material) => {
                        let container = match source {
                            SnapshotFileSource::SnapshotOnly => {
                                ContainerPublication::SnapshotOnly { file: file.file_id }
                            }
                            SnapshotFileSource::Container { branch, node } => {
                                match locate(file.file_id)? {
                                    ContainerPresence::Present { entity } => {
                                        ContainerPublication::Update {
                                            file: file.file_id,
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
                        };
                        Transfer::MoveWorking { material, container }
                    }
                    None => Transfer::CopyOriginal { file },
                },
            };
            result.push(PlannedEntry { path: &entry.path, transfer });
        }
        Ok(result)
    }

    /// After the permanent move/copy decision has completed, publish the new
    /// node's route while keeping every logical path. Never refill the worktree.
    pub(crate) fn published_view(
        &self,
        node: &AdmittedSnapshotManifest,
        published_sources: &[PublishedFileSource],
    ) -> Result<SnapshotReference, ProjectionError> {
        if published_sources.len() != self.edits.len()
            || node.repository_id() != self.source.repository_id()
            || node.node_id() == self.source.node_id()
            || node.entries().len() != self.source.entries().len()
        {
            return Err(ProjectionError::PublishedView);
        }
        for (index, (original, published)) in self.source.entries().iter()
            .zip(node.entries()).enumerate()
        {
            if original.path != published.path
                || self.final_content(index, original, published_sources)? != published.content
            {
                return Err(ProjectionError::PublishedView);
            }
        }
        Ok(node.reference())
    }

    fn final_content(
        &self,
        index: usize,
        entry: &SnapshotEntry,
        published_sources: &[PublishedFileSource],
    ) -> Result<SnapshotEntryContent, ProjectionError> {
        Ok(match entry.content {
            SnapshotEntryContent::Directory {} => SnapshotEntryContent::Directory {},
            SnapshotEntryContent::File { material: file, source } => match self.edits.get(&index) {
                Some(material) => {
                    let mut selected = published_sources.iter()
                        .filter(|item| item.path == entry.path);
                    let published_source = selected.next()
                        .ok_or(ProjectionError::PublishedView)?.source;
                    if selected.next().is_some() {
                        return Err(ProjectionError::PublishedView);
                    }
                    if source == SnapshotFileSource::SnapshotOnly
                        && published_source != SnapshotFileSource::SnapshotOnly
                    {
                        return Err(ProjectionError::PublishedView);
                    }
                    SnapshotEntryContent::File { material: material.file, source: published_source }
                }
                None => SnapshotEntryContent::File { material: file, source },
            },
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/projection.rs"]
mod tests;
