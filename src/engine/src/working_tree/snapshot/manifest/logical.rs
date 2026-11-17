//! The shared logical path/size gate for complete snapshots and pre-history
//! worktree additions. It neither invents a VersionId nor admits any BODY.
use super::{MAX_ENTRIES, MAX_LOGICAL_BYTES, MAX_PATH_BYTES, SnapshotManifestError};
use crate::persist::layout;
use name_policy::PortablePath;
use std::collections::BTreeMap;

pub(crate) struct LogicalAdmission<'a> {
    entries: BTreeMap<&'a str, (&'a PortablePath, bool)>,
    path_bytes: usize,
    logical_bytes: u64,
    management: String,
}

impl<'a> LogicalAdmission<'a> {
    pub(crate) fn new(count: usize) -> Result<Self, SnapshotManifestError> {
        if count > MAX_ENTRIES {
            return Err(SnapshotManifestError::Budget);
        }
        Ok(Self {
            entries: BTreeMap::new(),
            path_bytes: 0,
            logical_bytes: 0,
            management: name_policy::lookup_key(
                layout::SNAPSHOT_REPOSITORY
                    .as_os_str()
                    .to_str()
                    .expect("invariant: snapshot repository layout name is ASCII"),
            ),
        })
    }

    /// None is a directory; Some(size) is a file, including readable Empty.
    /// Call in strict canonical path order. Immutable version admission is
    /// deliberately separate, because an Added file has no version yet.
    pub(crate) fn entry(
        &mut self,
        path: &'a PortablePath,
        logical_size: Option<u64>,
    ) -> Result<(), SnapshotManifestError> {
        self.path_bytes = self
            .path_bytes
            .checked_add(path.as_str().len())
            .filter(|total| *total <= MAX_PATH_BYTES)
            .ok_or(SnapshotManifestError::Budget)?;
        if self
            .entries
            .last_key_value()
            .is_some_and(|(previous, _)| *previous >= path.key())
        {
            return Err(SnapshotManifestError::Order);
        }
        if path
            .key()
            .split('/')
            .any(|component| component == self.management)
        {
            return Err(SnapshotManifestError::Management);
        }
        if let Some(parent) = path.parent() {
            let (ancestor, directory) = self
                .entries
                .get(parent.key())
                .ok_or(SnapshotManifestError::Parent)?;
            if ancestor.as_str() != parent.as_str() || !*directory {
                return Err(SnapshotManifestError::Parent);
            }
        }
        if let Some(size) = logical_size {
            self.logical_bytes = self
                .logical_bytes
                .checked_add(size)
                .filter(|total| *total <= MAX_LOGICAL_BYTES)
                .ok_or(SnapshotManifestError::Budget)?;
        }
        self.entries
            .insert(path.key(), (path, logical_size.is_none()));
        Ok(())
    }

    pub(crate) fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }
}
