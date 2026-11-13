//! Existing-file sparse edits retain logical paths while physical bodies move.
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile, SnapshotReference,
};
use name_policy::{LookupPath, PortablePath};
use platform_io::fs_safety::EntryName;
use std::collections::BTreeMap;
use storage_contract::{FileId, VersionId};
use storage_contract::working_tree::LocalEntityIdentity;

/// Complete bytes already admitted from this instance's held sparse directory.
/// A leaf or a hash alone is not authority to read or move this material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkingFile {
    pub(crate) file: SnapshotFile,
    pub(crate) leaf: EntryName,
    pub(crate) entity: LocalEntityIdentity,
}

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
            let SnapshotEntryContent::File(original) = entry.content else {
                return Err(ProjectionError::Source);
            };
            if entry.path != edit.path || original.file_id != edit.material.file.file_id {
                return Err(ProjectionError::Source);
            }
            if changed(original, edit.material.file)
                && original.version_id == edit.material.file.version_id
            {
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
            SnapshotEntryContent::File(file) => match self.edits.get(&index) {
                Some(material) if changed(file, material.file) => ReadRoute::Working { material },
                Some(_) | None => ReadRoute::Original { file },
            },
        })
    }

    /// Resolve stable container identity only for actual modifications. The
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
                SnapshotEntryContent::File(file) => match self.edits.get(&index) {
                    Some(material) if changed(file, material.file) => {
                        let container = match locate(file.file_id)? {
                            ContainerPresence::Present { entity } => ContainerPublication::Update {
                                file: file.file_id,
                                source_version: file.version_id,
                                entity,
                            },
                            ContainerPresence::Absent => {
                                ContainerPublication::SnapshotOnly { file: file.file_id }
                            }
                        };
                        Transfer::MoveWorking { material, container }
                    }
                    Some(_) | None => Transfer::CopyOriginal { file },
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
    ) -> Result<SnapshotReference, ProjectionError> {
        if node.repository_id() != self.source.repository_id()
            || node.node_id() == self.source.node_id()
            || node.entries().len() != self.source.entries().len()
        {
            return Err(ProjectionError::PublishedView);
        }
        for (index, (original, published)) in self.source.entries().iter()
            .zip(node.entries()).enumerate()
        {
            if original.path != published.path
                || self.final_content(index, original) != published.content
            {
                return Err(ProjectionError::PublishedView);
            }
        }
        Ok(node.reference())
    }

    fn final_content(&self, index: usize, original: &SnapshotEntry) -> SnapshotEntryContent {
        match original.content {
            SnapshotEntryContent::Directory {} => SnapshotEntryContent::Directory {},
            SnapshotEntryContent::File(file) => match self.edits.get(&index) {
                Some(material) if changed(file, material.file) => {
                    SnapshotEntryContent::File(material.file)
                }
                Some(_) | None => SnapshotEntryContent::File(file),
            },
        }
    }
}

fn changed(original: SnapshotFile, current: SnapshotFile) -> bool {
    original.size != current.size || original.content_hash != current.content_hash
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/projection.rs"]
mod tests;
