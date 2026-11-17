//! Pre-history files join the same ordered logical projection without a fake
//! snapshot version/hash or a second BODY reader. Native original receipts own
//! their actual containers and working manifests.
use super::{Content, Entry, ProjectionError, SparseProjection};
use crate::directory_worktree::ledger::AddedFile;
use crate::working_tree::snapshot::{
    LogicalAdmission, MAX_ENTRIES, MAX_LOGICAL_BYTES, MAX_VERSIONS,
};
use std::collections::BTreeSet;
use storage_contract::file_draft::UnbornBody;
use storage_contract::{EntryId, FileId, OperationId};

impl SparseProjection<'_> {
    pub(crate) fn with_added(mut self, files: &[AddedFile]) -> Result<Self, ProjectionError> {
        if !self.added.is_empty() {
            return Err(ProjectionError::Added);
        }
        if self
            .entries
            .len()
            .checked_add(files.len())
            .is_none_or(|count| count > MAX_ENTRIES)
        {
            return Err(ProjectionError::Material(
                crate::working_tree::snapshot::SnapshotManifestError::Budget,
            ));
        }
        let mut identities = self
            .source
            .files()
            .values()
            .map(|file| file.file_id)
            .collect::<BTreeSet<FileId>>();
        let mut births = BTreeSet::<OperationId>::new();
        let mut handle = None;
        for file in files {
            if handle.is_some_and(|target| target != file.target.worktree)
                || !identities.insert(file.target.file)
                || !births.insert(file.birth)
                || self.added.contains_key(&file.target.entry)
            {
                return Err(ProjectionError::Added);
            }
            handle = Some(file.target.worktree);
            if let UnbornBody::Saved { manifest } = file.working.body() {
                manifest
                    .validate(MAX_LOGICAL_BYTES)
                    .map_err(ProjectionError::WorkingManifest)?;
                if manifest.file() != file.target.file {
                    return Err(ProjectionError::Added);
                }
            }
            let index = match self
                .entries
                .binary_search_by(|entry| entry.path.key().cmp(file.target.path.key()))
            {
                Ok(_) => return Err(ProjectionError::Added),
                Err(index) => index,
            };
            self.entries.insert(
                index,
                Entry {
                    path: file.target.path.clone(),
                    content: Content::Added {
                        entry: file.target.entry,
                    },
                },
            );
            self.added.insert(file.target.entry, file.clone());
        }
        self.logical_admission(None)?;
        Ok(self)
    }

    /// Native write preparation can cap one exact Added file by the remaining
    /// complete snapshot budget. The final save repeats the full admission.
    pub(crate) fn added_file_budget(&self, entry: EntryId) -> Result<u64, ProjectionError> {
        if !self.added.contains_key(&entry) {
            return Err(ProjectionError::Added);
        }
        let other_bytes = self.logical_admission(Some(entry))?;
        Ok(MAX_LOGICAL_BYTES - other_bytes)
    }

    fn logical_admission(&self, exclude_size: Option<EntryId>) -> Result<u64, ProjectionError> {
        let mut admission =
            LogicalAdmission::new(self.entries.len()).map_err(ProjectionError::Material)?;
        let mut versions = BTreeSet::new();
        for entry in &self.entries {
            let size = match entry.content {
                Content::Directory => None,
                Content::File {
                    original, material, ..
                } => {
                    let selected = match self.edits.get(&original) {
                        Some(edit) => edit.file,
                        None => material,
                    };
                    versions.insert(selected.version_id);
                    Some(selected.size)
                }
                Content::Added { entry } => Some(if exclude_size == Some(entry) {
                    0
                } else {
                    self.added[&entry].working.logical_size()
                }),
            };
            admission
                .entry(&entry.path, size)
                .map_err(ProjectionError::Material)?;
        }
        // Each new independent container will need its own first version; do
        // not accept a normal save that cannot fit the next complete snapshot.
        if versions.len() + self.added.len() > MAX_VERSIONS {
            return Err(ProjectionError::Material(
                crate::working_tree::snapshot::SnapshotManifestError::Budget,
            ));
        }
        Ok(admission.logical_bytes())
    }
}
