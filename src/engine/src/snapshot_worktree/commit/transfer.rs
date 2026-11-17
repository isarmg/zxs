//! Same no-replace node publisher for the original edit and a true Added FULL.
//! Only the commit owner constructs this after verifying its independent FILE
//! result. Descriptive records alone never supply either held source below.
use crate::container_storage::PreparedUnbornFull;
use crate::directory_worktree::ledger::AddedFileResult;
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::persist::snapshot_worktree::CommitReservation;
use crate::persist::snapshot_worktree::added_commit::{AddedCommit, AddedFirst};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotFile, SnapshotReference, SnapshotStoreError,
};
use std::sync::Arc;

enum Material<'a> {
    Existing {
        instance: HeldSparseWorktree,
        plan: &'a CommitReservation,
    },
    Added {
        full: Arc<PreparedUnbornFull>,
        result: &'a AddedFileResult,
    },
}

pub(crate) struct ReservedDirectoryTransfer<'a> {
    original: &'a HeldSnapshotNode,
    source: SnapshotReference,
    expected: SnapshotReference,
    material: Material<'a>,
}

impl<'a> ReservedDirectoryTransfer<'a> {
    pub(super) fn existing(
        original: &'a HeldSnapshotNode,
        instance: HeldSparseWorktree,
        plan: &'a CommitReservation,
        expected: SnapshotReference,
    ) -> Self {
        Self {
            original,
            source: plan.binding.source(),
            expected,
            material: Material::Existing { instance, plan },
        }
    }

    pub(super) fn added(
        original: &'a HeldSnapshotNode,
        plan: &AddedCommit,
        first: &AddedFirst,
        result: &'a AddedFileResult,
        full: Arc<PreparedUnbornFull>,
        expected: SnapshotReference,
    ) -> Result<Self, SnapshotStoreError> {
        if first.reserve != plan.request.operation_id
            || full.worktree_evidence().map_err(SnapshotStoreError::Disk)? != first.full
            || result.target != plan.added.target
            || result.birth != plan.added.birth
            || result.working != plan.added.working
            || result.file_operation != plan.file_operation
            || first.content.version.file_id != result.material.file_id.to_string()
            || first.content.version.version_id != result.material.version_id.to_string()
            || first.content.version.content_hash != result.material.content_hash
            || first.content.version.logical_size != result.material.size
        {
            return Err(SnapshotStoreError::Version);
        }
        Ok(Self {
            original,
            source: plan.binding.source(),
            expected,
            material: Material::Added { full, result },
        })
    }

    pub(crate) fn original(&self) -> &HeldSnapshotNode {
        self.original
    }

    pub(crate) fn require_manifest(
        &self,
        manifest: &AdmittedSnapshotManifest,
    ) -> Result<(), SnapshotStoreError> {
        if manifest.reference() != self.expected
            || self.original.manifest().reference() != self.source
        {
            return Err(SnapshotStoreError::Repository);
        }
        Ok(())
    }

    pub(crate) fn transfer(
        &self,
        target: &crate::disk::DiskRoot,
        file: &SnapshotFile,
        name: &platform_io::fs_safety::EntryName,
    ) -> Result<(), SnapshotStoreError> {
        match &self.material {
            Material::Existing { instance, plan }
                if file.version_id == plan.edit.material.version_id =>
            {
                if *file != plan.edit.material {
                    return Err(SnapshotStoreError::Version);
                }
                instance
                    .directory()
                    .move_new_entry_verified_synced(
                        plan.edit.leaf.entry(),
                        target,
                        name,
                        crate::disk::FrozenFileMove {
                            bytes: file.size,
                            content_hash: file.content_hash,
                            entity: plan.edit.entity,
                        },
                    )
                    .map_err(SnapshotStoreError::Move)?;
                return Ok(());
            }
            Material::Added { full, result } if file.version_id == result.material.version_id => {
                if *file != result.material {
                    return Err(SnapshotStoreError::Version);
                }
                return full.move_worktree_full(target, name, file);
            }
            Material::Existing { .. } | Material::Added { .. } => {}
        }
        if self.original.manifest().files().get(&file.version_id) != Some(file) {
            return Err(SnapshotStoreError::Version);
        }
        target
            .copy_new_entry_verified_synced(
                name,
                &mut self.original.open_payload(file.version_id)?,
                platform_io::payload::Expectation {
                    bytes: file.size,
                    blake3: Some(file.content_hash.digest()),
                },
            )
            .map_err(SnapshotStoreError::Copy)?;
        Ok(())
    }
}
