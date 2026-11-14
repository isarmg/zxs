//! FILE obligations are keyed by original occurrences, not final snapshot paths.
//!
//! This immutable planning view is not a publication decision or receipt proof.
//! Before publication its owner must bind worktree identity, revision, operation
//! and the authoritative ledger checkpoint in one durable envelope. Namespace
//! Commit/Delete fences remain necessary until independent receipt closure exists.
use crate::directory_worktree::projection::namespace::Content;
use crate::directory_worktree::projection::{SparseProjection, WorkingFile};
use crate::working_tree::snapshot::{
    SnapshotEntryContent, SnapshotFile, SnapshotFileSource, SnapshotReference,
};
use name_policy::PortablePath;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileBaseline {
    material: SnapshotFile,
    source: SnapshotFileSource,
}
impl FileBaseline {
    pub(crate) fn material(self) -> SnapshotFile { self.material }
    pub(crate) fn source(self) -> SnapshotFileSource { self.source }
}

/// An occurrence within this exact source, not a worktree/publication identity.
/// Aliases may share FileId and version while selecting different branch pins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OriginalOccurrence<'a> {
    snapshot: SnapshotReference,
    path: &'a PortablePath,
    baseline: FileBaseline,
}
impl<'a> OriginalOccurrence<'a> {
    pub(crate) fn snapshot(self) -> SnapshotReference { self.snapshot }
    pub(crate) fn path(self) -> &'a PortablePath { self.path }
    pub(crate) fn baseline(self) -> FileBaseline { self.baseline }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkingBodyDisposition<'a> {
    /// Logical destination only. Physical COPY/MOVE authority is separate.
    Visible { path: &'a PortablePath },
    /// Keep the exact sealed leaf/entity in this worktree instance. A FILE
    /// receipt never authorizes deleting it or moving it to another location.
    RetainedInWorktree,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FilePublicationRequirement {
    /// The latest explicit write still needs an independent completion result,
    /// including SnapshotOnly outcomes and originals hidden from the final view.
    Pending { baseline: FileBaseline },
    /// The owner supplied a result for this exact occurrence and version.
    /// Structural admission here does not verify its independent receipt.
    Recorded { result: FileBaseline },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlannedWorkingFile<'a> {
    original: OriginalOccurrence<'a>,
    material: &'a WorkingFile,
    body: WorkingBodyDisposition<'a>,
    publication: FilePublicationRequirement,
}
impl<'a> PlannedWorkingFile<'a> {
    pub(crate) fn original(self) -> OriginalOccurrence<'a> { self.original }
    pub(crate) fn material(self) -> &'a WorkingFile { self.material }
    pub(crate) fn body(self) -> WorkingBodyDisposition<'a> { self.body }
    pub(crate) fn publication(self) -> FilePublicationRequirement { self.publication }
}

/// The immutable borrow freezes source pins, current material and disposition
/// together. Enumeration is in original path order, never FileId/hash order.
pub(crate) struct FileObligationPlan<'a> {
    files: BTreeMap<usize, PlannedWorkingFile<'a>>,
}
impl<'a> FileObligationPlan<'a> {
    pub(crate) fn files(&self) -> impl ExactSizeIterator<Item = &PlannedWorkingFile<'a>> {
        self.files.values()
    }
    pub(super) fn for_original(&self, index: usize) -> Option<&PlannedWorkingFile<'a>> {
        self.files.get(&index)
    }
}

impl SparseProjection<'_> {
    /// Cover every admitted edit, including hidden originals and already saved
    /// bodies. Only an exact saved version suppresses its next FILE obligation;
    /// neither namespace removal nor matching content suppresses an edit.
    pub(crate) fn file_obligations(&self) -> FileObligationPlan<'_> {
        if self.edits.is_empty() {
            return FileObligationPlan { files: BTreeMap::new() };
        }
        let visible = self.entries.iter().filter_map(|entry| match entry.content {
            Content::Directory => None,
            Content::File { original, .. } => self.edits.contains_key(&original)
                .then_some((original, &entry.path)),
        }).collect::<BTreeMap<_, _>>();
        // Admission binds ordered edit keys to exact source files, including
        // originals hidden from the namespace projection.
        let files = self.edits.iter().filter_map(|(&index, material)| {
            let entry = &self.source.entries()[index];
            let (original_material, source) = match entry.content {
                SnapshotEntryContent::Directory {} => return None,
                SnapshotEntryContent::File { material, source } => (material, source),
            };
            let baseline = FileBaseline { material: original_material, source };
            let publication = match self.results.get(&index) {
                Some(saved) => {
                    let result = FileBaseline { material: saved.material, source: saved.source };
                    if saved.material.version_id == material.file.version_id {
                        FilePublicationRequirement::Recorded { result }
                    } else {
                        FilePublicationRequirement::Pending { baseline: result }
                    }
                }
                None => FilePublicationRequirement::Pending { baseline },
            };
            let body = match visible.get(&index).copied() {
                Some(path) => WorkingBodyDisposition::Visible { path },
                None => WorkingBodyDisposition::RetainedInWorktree,
            };
            Some((index, PlannedWorkingFile {
                original: OriginalOccurrence {
                    snapshot: self.source.reference(), path: &entry.path, baseline,
                },
                material,
                body,
                publication,
            }))
        }).collect();
        FileObligationPlan { files }
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/directory_worktree/obligations.rs"]
mod tests;
