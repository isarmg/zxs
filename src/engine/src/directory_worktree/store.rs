//! Sparse worktrees start with no files and retain edits independently of source.
use crate::directory_worktree::lifecycle::WorktreeId;
use crate::directory_worktree::projection::WorkingFile;
use std::io::Seek;
use crate::disk::{DirectoryCreateOutcome, DirectoryIoError, DiskRoot};
use crate::persist::layout::worktree as layout;
use crate::working_tree::records::{file_identity, local_entity};
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotReference};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;
use storage_contract::OperationId;
use storage_contract::working_tree::LocalEntityIdentity;

#[derive(Debug, error_definition::Error)]
pub(crate) enum WorktreeStoreError {
    #[error("worktree directory access failed: {0}")]
    Directory(#[source] DirectoryIoError),
    #[error("worktree disk access failed: {0}")]
    Disk(#[source] crate::Error),
    #[error("worktree belongs to another original snapshot or parent directory")]
    Source,
    #[error("worktree edited payload verification failed")]
    Payload(#[source] platform_io::payload::Error),
    #[error("worktree edited payload I/O failed")]
    Io(#[source] std::io::Error),
    #[error("worktree edited payload differs from its sealed physical entity")]
    MaterialIdentity,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum WorktreePreparationError {
    #[error("worktree creation was rejected before creating its directory")]
    Rejected(#[source] WorktreeStoreError),
    #[error("worktree creation is incomplete; retain the original instance identity")]
    Incomplete {
        worktree: WorktreeId,
        operation: OperationId,
        entity: Option<LocalEntityIdentity>,
        #[source]
        source: WorktreeStoreError,
    },
}

/// Persist this exact binding in the independently decided worktree ledger.
/// A directory or a self-reported JSON record is not recovery authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SparseWorktreeBinding {
    worktree: WorktreeId,
    operation: OperationId,
    source: SnapshotReference,
    parent_entity: LocalEntityIdentity,
    worktree_entity: LocalEntityIdentity,
}
impl SparseWorktreeBinding {
    pub(crate) fn worktree(self) -> WorktreeId {
        self.worktree
    }
    pub(crate) fn operation(self) -> OperationId {
        self.operation
    }
    pub(crate) fn source(self) -> SnapshotReference {
        self.source
    }
}

pub(crate) struct HeldSparseWorktree {
    binding: SparseWorktreeBinding,
    directory: DiskRoot,
}
impl HeldSparseWorktree {
    /// The caller holds the repository writer fence and durably reserves a new
    /// identity. No source body is opened or copied. SMB initially reads original
    /// node references; only later edits may install files in this directory.
    pub(crate) fn prepare(
        source: &HeldSnapshotNode,
        worktree: WorktreeId,
        operation: OperationId,
    ) -> Result<Self, WorktreePreparationError> {
        let parent = source.worktrees_directory();
        let _activity = parent.retain_io_activity()
            .map_err(WorktreeStoreError::Disk)
            .map_err(WorktreePreparationError::Rejected)?;
        let parent_entity = parent.held_identity()
            .map(local_entity)
            .map_err(WorktreeStoreError::Directory)
            .map_err(WorktreePreparationError::Rejected)?;
        let (directory, identity) = match parent.create_private_entry(&layout::instance(worktree)) {
            DirectoryCreateOutcome::Rejected(source) => {
                return Err(WorktreePreparationError::Rejected(
                    WorktreeStoreError::Directory(source),
                ));
            }
            DirectoryCreateOutcome::Uncertain { identity, source } => {
                return Err(incomplete(worktree, operation, identity, source));
            }
            DirectoryCreateOutcome::Created { directory, identity, durability } => {
                durability.map_err(|source| {
                    incomplete(worktree, operation, Some(identity), source)
                })?;
                (directory, identity)
            }
        };
        // create_private_entry already synchronizes both the new empty directory
        // and its parent. No marker or initial body is written into the worktree.
        Ok(Self {
            binding: SparseWorktreeBinding {
                worktree,
                operation,
                source: source.manifest().reference(),
                parent_entity,
                worktree_entity: local_entity(identity),
            },
            directory,
        })
    }

    /// Recover the exact original instance, including its current sparse edits.
    /// Do not initialize, reset, copy from the source, or adopt another instance.
    pub(crate) fn reopen(
        source: &HeldSnapshotNode,
        expected: SparseWorktreeBinding,
    ) -> Result<Self, WorktreeStoreError> {
        let parent = source.worktrees_directory();
        if source.manifest().reference() != expected.source
            || parent.held_identity().map_err(WorktreeStoreError::Directory)?
                != file_identity(expected.parent_entity)
        {
            return Err(WorktreeStoreError::Source);
        }
        let directory = parent.private_entry_matching(
            &layout::instance(expected.worktree),
            file_identity(expected.worktree_entity),
        ).map_err(WorktreeStoreError::Directory)?;
        Ok(Self { binding: expected, directory })
    }

    pub(crate) fn binding(&self) -> SparseWorktreeBinding {
        self.binding
    }

    /// Resolve only the exact previously sealed edit. The file remains managed
    /// by its original disk gate; no raw descriptor or external path escapes.
    pub(crate) fn open_material(&self, material: &WorkingFile)
        -> Result<crate::ReadFile, WorktreeStoreError>
    {
        let mut file = self.directory.open_read_entry(&material.leaf)
            .map_err(WorktreeStoreError::Disk)?;
        let verified = file.verify_full(platform_io::payload::Expectation {
            bytes: material.file.size, blake3: Some(material.file.content_hash.digest()),
        }).map_err(WorktreeStoreError::Payload)?;
        if verified.identity != file_identity(material.entity) {
            return Err(WorktreeStoreError::MaterialIdentity);
        }
        file.rewind().map_err(WorktreeStoreError::Io)?;
        Ok(file)
    }

    /// Mutable sparse material, not a complete directory listing or permission
    /// to publish an SMB mount. Its owner supplies lifecycle and overlay facts.
    pub(crate) fn directory(&self) -> &DiskRoot {
        &self.directory
    }
}

fn incomplete(
    worktree: WorktreeId,
    operation: OperationId,
    identity: Option<FileIdentity>,
    source: DirectoryIoError,
) -> WorktreePreparationError {
    WorktreePreparationError::Incomplete {
        worktree,
        operation,
        entity: identity.map(local_entity),
        source: WorktreeStoreError::Directory(source),
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/store.rs"]
mod tests;
