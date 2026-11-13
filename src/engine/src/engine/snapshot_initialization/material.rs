//! Held physical material only. The caller retains the writer fence and must
//! verify its permanent decision before installation; this module grants none.
use super::{bootstrap::AdmittedFreshRoot, intent::{AdmittedIntent, IntentError}};
use crate::disk::{DiskHandle, DiskRoot, DirectoryCreateOutcome, DirectoryInstallOutcome,
    DirectoryIoError, WorkingDirectory};
use crate::persist::layout;
use crate::working_tree::admission::{AdmissionError, RootSelectionContext,
    select_root, validate_catalog_structure};
use crate::working_tree::records::file_identity;
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotFile, SnapshotNodeBinding,
    SnapshotPreparationError, SnapshotStoreError};
use platform_io::fs_safety::EntryName;
use std::io::Read;
use storage_contract::OperationId;
use storage_contract::working_tree::{DirectoryTargetPath, RootBinding, WorkingTreeCatalog};

type Result<T> = std::result::Result<T, MaterialError>;
#[derive(Debug, error_definition::Error)]
pub(crate) enum MaterialError {
    #[error("directory material differs from its independently admitted fresh root")]
    Binding,
    #[error("directory material requires the primary common-data root target")]
    Target,
    #[error("directory material disk I/O failed")]
    Disk(#[source] crate::Error),
    #[error("directory material filesystem access failed")]
    Directory(#[source] DirectoryIoError),
    #[error("directory material name failed")]
    Name(#[source] platform_io::fs_safety::Error),
    #[error("directory material structural closure failed")]
    Structure(#[source] AdmissionError),
    #[error("directory material intent failed")]
    Intent(#[source] IntentError),
    #[error("directory snapshot preparation failed; retain original material")]
    Preparation(#[source] SnapshotPreparationError),
    #[error("directory snapshot verification failed")]
    Snapshot(#[source] SnapshotStoreError),
    #[error("directory material may have been created or installed; retain original material")]
    Uncertain(#[source] DirectoryIoError),
}

pub(crate) struct PreparedMaterial {
    root: RootBinding,
    operation: OperationId,
    parent: DiskRoot,
    repository: DiskRoot,
    node: HeldSnapshotNode,
    installed: bool,
}

fn stage_name(operation: OperationId) -> Result<EntryName> {
    Ok(layout::snapshot_init::stage(operation))
}

fn roots(admitted: &AdmittedFreshRoot, disk: &DiskHandle) -> Result<(DiskRoot, DiskRoot)> {
    let root = admitted.binding();
    let registration = &root.registration;
    if !root.source_chain.is_empty()
        || registration.pool_id != disk.seal.pool_id
        || registration.disk_id != disk.seal.disk_id
        || registration.slot_id != disk.seal.slot_id
        || registration.binding_generation != disk.seal.binding_generation
        || disk.root.held_identity().map_err(MaterialError::Directory)?
            != file_identity(root.root_entity)
    {
        return Err(MaterialError::Binding);
    }
    let common = disk.root.private_entry_matching(&layout::COMMON_DATA_ROOT,
        file_identity(root.roots.common_data.entity)).map_err(MaterialError::Directory)?;
    let _deleted = disk.root.private_entry_matching(&layout::DELETED_DATA_ROOT,
        file_identity(root.roots.deleted_data.entity)).map_err(MaterialError::Directory)?;
    let zxs = disk.root.private_entry_matching(&layout::ZXS_DATA_ROOT,
        file_identity(root.roots.zxs_data.entity)).map_err(MaterialError::Directory)?;
    Ok((common, zxs))
}

/// A's explicit source initializer targets a new empty common root. It never
/// classifies or adopts ordinary host files; those need later catalog producers.
pub(crate) fn require_empty_target(admitted: &AdmittedFreshRoot, disk: &DiskHandle) -> Result<()> {
    let (common, _) = roots(admitted, disk)?;
    require_host_inventory(&common, false)
}

fn require_host_inventory(common: &DiskRoot, installed: bool) -> Result<()> {
    let mut count = 0usize;
    for entry in common.root_entries().map_err(MaterialError::Directory)? {
        let entry = entry.map_err(MaterialError::Name)?;
        if !installed || entry != *layout::SNAPSHOT_REPOSITORY || count != 0 {
            return Err(MaterialError::Target);
        }
        count += 1;
    }
    if installed && count != 1 { return Err(MaterialError::Target); }
    Ok(())
}

pub(crate) fn prepare<R: Read>(
    admitted: &AdmittedFreshRoot,
    disk: &DiskHandle,
    intent: &AdmittedIntent,
    source: impl FnMut(&SnapshotFile) -> std::result::Result<R, SnapshotStoreError>,
) -> Result<PreparedMaterial> {
    let _activity = disk.root.retain_io_activity().map_err(MaterialError::Disk)?;
    let record = intent.record();
    let root = admitted.binding();
    if record.root != *root || record.host_entity != root.roots.common_data.entity {
        return Err(MaterialError::Binding);
    }
    if record.target.path != DirectoryTargetPath::Root {
        return Err(MaterialError::Target);
    }
    require_empty_target(admitted, disk)?;
    let manifest = intent.manifest().map_err(MaterialError::Intent)?;
    let (_, parent) = roots(admitted, disk)?;
    let name = stage_name(record.operation)?;
    let repository = match parent.create_private_entry(&name) {
        DirectoryCreateOutcome::Rejected(error) => return Err(MaterialError::Directory(error)),
        DirectoryCreateOutcome::Uncertain { source, .. } => {
            return Err(MaterialError::Uncertain(source));
        }
        DirectoryCreateOutcome::Created { directory, durability, .. } => {
            durability.map_err(MaterialError::Uncertain)?;
            directory
        }
    };
    let node = HeldSnapshotNode::prepare(&repository, manifest, source)
        .map_err(MaterialError::Preparation)?;
    roots(admitted, disk)?;
    parent.private_entry_matching(&name, file_identity(node.binding().repository_entity()))
        .map_err(MaterialError::Directory)?;
    Ok(PreparedMaterial {
        root: root.clone(), operation: record.operation, parent, repository, node, installed: false,
    })
}

pub(crate) fn reopen_stage(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId,
) -> Result<PreparedMaterial> {
    reopen(admitted, disk, original, operation, false)
}

pub(crate) fn reopen_installed(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId,
) -> Result<PreparedMaterial> {
    reopen(admitted, disk, original, operation, true)
}

/// Only a decided caller may use this recovery choice. Errors never mean absent.
pub(crate) fn reopen_decided(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId,
) -> Result<PreparedMaterial> {
    let _activity = disk.root.retain_io_activity().map_err(MaterialError::Disk)?;
    let (common, _) = roots(admitted, disk)?;
    let installed = common.optional_metadata(
        &common.path().join(layout::SNAPSHOT_REPOSITORY.as_path()),
    ).map_err(MaterialError::Disk)?.is_some();
    if installed {
        reopen_installed(admitted, disk, original, operation)
    } else {
        reopen_stage(admitted, disk, original, operation)
    }
}

fn reopen(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId, installed: bool,
) -> Result<PreparedMaterial> {
    let _activity = disk.root.retain_io_activity().map_err(MaterialError::Disk)?;
    let (common, parent) = roots(admitted, disk)?;
    require_host_inventory(&common, installed)?;
    let name = if installed { layout::SNAPSHOT_REPOSITORY.clone() } else { stage_name(operation)? };
    let holder = if installed { &common } else { &parent };
    let repository = holder.private_entry_matching(&name,
        file_identity(original.repository_entity())).map_err(MaterialError::Directory)?;
    let expected_node = layout::snapshot::node(original.reference().node);
    let mut nodes = 0usize;
    let mut count = 0usize;
    for entry in repository.root_entries().map_err(MaterialError::Directory)? {
        let entry = entry.map_err(MaterialError::Name)?;
        count += 1;
        if count > 2 { return Err(MaterialError::Target); }
        if entry == expected_node {
            nodes += 1;
        } else if !installed || entry != *layout::snapshot_init::DECISION {
            return Err(MaterialError::Target);
        }
    }
    if nodes != 1 { return Err(MaterialError::Target); }
    let node = HeldSnapshotNode::reopen(&repository, original).map_err(MaterialError::Snapshot)?;
    roots(admitted, disk)?;
    holder.private_entry_matching(&name, file_identity(original.repository_entity()))
        .map_err(MaterialError::Directory)?;
    Ok(PreparedMaterial {
        root: admitted.binding().clone(), operation, parent, repository, node, installed,
    })
}

impl PreparedMaterial {
    pub(crate) fn repository(&self) -> &DiskRoot {
        &self.repository
    }

    pub(crate) fn node(&self) -> &HeldSnapshotNode {
        &self.node
    }

    pub(crate) fn binding(&self) -> SnapshotNodeBinding {
        self.node.binding()
    }

    /// Caller must already possess the permanent decision for this exact binding.
    /// No marker, stage filename or structural catalog is publication authority.
    pub(crate) fn install(&mut self, admitted: &AdmittedFreshRoot, disk: &DiskHandle) -> Result<()> {
        let _activity = disk.root.retain_io_activity().map_err(MaterialError::Disk)?;
        if self.root != *admitted.binding() {
            return Err(MaterialError::Binding);
        }
        roots(admitted, disk)?;
        if self.installed {
            *self = reopen_installed(admitted, disk, self.binding(), self.operation)?;
            return Ok(());
        }
        // The only row comes from SYSTEM bootstrap admission, not from disk JSON.
        // Root-only traversal needs no ordinary-directory or FILE inventory rows.
        let catalog = WorkingTreeCatalog {
            storage_layout: self.root.layout,
            roots: vec![self.root.clone()],
            directories: vec![], containers: vec![], snapshot_repositories: vec![], replicas: vec![],
        };
        let view = validate_catalog_structure(&catalog).map_err(MaterialError::Structure)?;
        let current = RootSelectionContext {
            storage_layout: admitted.binding().layout,
            registration: &admitted.binding().registration,
            source_chain: &admitted.binding().source_chain,
        };
        let selected = select_root(&view, self.root.root_id, &current)
            .map_err(MaterialError::Structure)?;
        let host = WorkingDirectory::open(disk, selected, &DirectoryTargetPath::Root)
            .map_err(MaterialError::Directory)?;
        HeldSnapshotNode::reopen(&self.repository, self.binding()).map_err(MaterialError::Snapshot)?;
        match host.install_repository(&self.parent, &stage_name(self.operation)?,
            &self.repository, 100_000)
        {
            DirectoryInstallOutcome::Rejected(error) => return Err(MaterialError::Directory(error)),
            DirectoryInstallOutcome::Uncertain(error) => return Err(MaterialError::Uncertain(error)),
            DirectoryInstallOutcome::Installed { identity, durability } => {
                self.installed = true;
                durability.map_err(MaterialError::Uncertain)?;
                if identity != file_identity(self.binding().repository_entity()) {
                    return Err(MaterialError::Binding);
                }
            }
        }
        host.revalidate().map_err(MaterialError::Directory)?;
        roots(admitted, disk)?;
        // Drop the structurally borrowed host before replacing owned root records.
        drop(host);
        *self = reopen_installed(admitted, disk, self.binding(), self.operation)?;
        Ok(())
    }
}
