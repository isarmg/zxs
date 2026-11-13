//! Held physical material only. The caller retains the writer fence and must
//! verify its permanent decision before installation; this module grants none.
use super::{bootstrap::AdmittedFreshRoot, intent::{AdmittedIntent, IntentError, SnapshotInitIntent}};
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
use storage_contract::working_tree::{RootBinding, WorkingTreeCatalog};

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
    intent: SnapshotInitIntent,
    _host_pins: Vec<DiskRoot>,
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

/// Build traversal structure exclusively from the exact persisted intent.
/// Syntax is not authority: callers separately admit the original root and
/// permanent decision (or the still-held first-preparation source).
fn host_catalog(record: &SnapshotInitIntent) -> WorkingTreeCatalog {
    let mut directories = record.directories.clone();
    directories.sort_by_key(|directory| directory.directory_id);
    WorkingTreeCatalog {
        storage_layout: record.root.layout,
        roots: vec![record.root.clone()],
        directories,
        containers: Vec::new(),
        snapshot_repositories: Vec::new(),
        replicas: Vec::new(),
    }
}

fn with_host<T>(
    admitted: &AdmittedFreshRoot,
    disk: &DiskHandle,
    record: &SnapshotInitIntent,
    action: impl FnOnce(&WorkingDirectory<'_>) -> Result<T>,
) -> Result<T> {
    if record.root != *admitted.binding() {
        return Err(MaterialError::Binding);
    }
    roots(admitted, disk)?;
    let catalog = host_catalog(record);
    let view = validate_catalog_structure(&catalog).map_err(MaterialError::Structure)?;
    let current = RootSelectionContext {
        storage_layout: record.root.layout,
        registration: &record.root.registration,
        source_chain: &record.root.source_chain,
    };
    let selected = select_root(&view, record.root.root_id, &current)
        .map_err(MaterialError::Structure)?;
    let host = WorkingDirectory::open(disk, selected, &record.target.path)
        .map_err(MaterialError::Directory)?;
    if host.registered_host_id() != Some(record.host_directory_id)
        || host.identity().map_err(MaterialError::Directory)? != file_identity(record.host_entity)
    {
        return Err(MaterialError::Binding);
    }
    let result = action(&host)?;
    host.revalidate().map_err(MaterialError::Directory)?;
    Ok(result)
}

pub(crate) fn require_empty_target(
    admitted: &AdmittedFreshRoot,
    disk: &DiskHandle,
    record: &SnapshotInitIntent,
) -> Result<()> {
    with_host(admitted, disk, record, |host| {
        if host.probe_repository(100_000).map_err(MaterialError::Directory)?
            != crate::disk::RepositoryLeaf::Absent
        {
            return Err(MaterialError::Target);
        }
        Ok(())
    })
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
    if record.root != *root {
        return Err(MaterialError::Binding);
    }
    require_empty_target(admitted, disk, record)?;
    let host_pins = with_host(admitted, disk, record, |host| {
        host.retain_chain().map_err(MaterialError::Directory)
    })?;
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
    with_host(admitted, disk, record, |_| Ok(()))?;
    Ok(PreparedMaterial {
        root: root.clone(), intent: record.clone(), _host_pins: host_pins,
        operation: record.operation,
        parent, repository, node, installed: false,
    })
}

pub(crate) fn reopen_stage(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId, record: &SnapshotInitIntent,
) -> Result<PreparedMaterial> {
    reopen(admitted, disk, original, operation, record, false)
}

pub(crate) fn reopen_installed(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId, record: &SnapshotInitIntent,
) -> Result<PreparedMaterial> {
    reopen(admitted, disk, original, operation, record, true)
}

/// Only a decided caller may use this recovery choice. Errors never mean absent.
pub(crate) fn reopen_decided(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId, record: &SnapshotInitIntent,
) -> Result<PreparedMaterial> {
    let _activity = disk.root.retain_io_activity().map_err(MaterialError::Disk)?;
    let common = with_host(admitted, disk, record, |host| {
        host.retain_host().map_err(MaterialError::Directory)
    })?;
    let installed = common.optional_metadata(
        &common.path().join(layout::SNAPSHOT_REPOSITORY.as_path()),
    ).map_err(MaterialError::Disk)?.is_some();
    if installed {
        reopen_installed(admitted, disk, original, operation, record)
    } else {
        reopen_stage(admitted, disk, original, operation, record)
    }
}

fn reopen(
    admitted: &AdmittedFreshRoot, disk: &DiskHandle,
    original: SnapshotNodeBinding, operation: OperationId, record: &SnapshotInitIntent, installed: bool,
) -> Result<PreparedMaterial> {
    let _activity = disk.root.retain_io_activity().map_err(MaterialError::Disk)?;
    if record.operation != operation || record.material != original.reference() {
        return Err(MaterialError::Binding);
    }
    let host_pins = with_host(admitted, disk, record, |host| {
        host.retain_chain().map_err(MaterialError::Directory)
    })?;
    let (_, parent) = roots(admitted, disk)?;
    let common = with_host(admitted, disk, record, |host| {
        host.retain_host().map_err(MaterialError::Directory)
    })?;
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
    with_host(admitted, disk, record, |_| Ok(()))?;
    Ok(PreparedMaterial {
        root: admitted.binding().clone(), intent: record.clone(), _host_pins: host_pins,
        operation, parent, repository, node, installed,
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
            *self = reopen_installed(admitted, disk, self.binding(), self.operation, &self.intent)?;
            return Ok(());
        }
        let record = self.intent.clone();
        with_host(admitted, disk, &record, |host| {
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
            Ok(())
        })?;
        *self = reopen_installed(admitted, disk, self.binding(), self.operation, &self.intent)?;
        Ok(())
    }
}
