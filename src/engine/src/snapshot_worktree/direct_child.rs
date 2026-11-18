//! An already issued ordinary direct child of a completed snapshot root.
//! This path can borrow an original Mkdir binding, never create, replace or
//! adopt a user's directory merely because its name exists.
use super::{ReceiptIndex, RegistrationError, Result};
use crate::container_storage::namespace_host::{NamespaceHostError, ordinary_path};
use crate::persist::snapshot_worktree::{HostEvidence, NamespaceDirectoryBinding};
use crate::working_tree::records::file_identity;
use crate::{Engine, NamespaceEntryKind, NamespaceSnapshot};
use name_policy::{LookupPath, PortablePath};
use storage_contract::file_container::NamespaceDirectoryParent;
use storage_contract::snapshot_worktree::discovery::{SnapshotBranchSelection, SnapshotHeadDiscovery, SnapshotHeadRequest};
use storage_contract::snapshot_worktree::{NamespaceDirectoryId, NamespaceHostAddress, SnapshotHost, SnapshotRootContext};
use storage_contract::{EntryId, RepositoryId};

pub(super) struct DirectChild {
    pub(super) context: SnapshotRootContext,
    pub(super) directory: NamespaceDirectoryBinding,
}

impl DirectChild {
    pub(super) fn capture(engine: &Engine, index: &ReceiptIndex<'_>, repository: RepositoryId,
        child: &LookupPath) -> Result<Self> {
        if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
            || engine.config.generation().is_some() || child.strip_components(1).is_some()
        {
            return Err(RegistrationError::RootOnly);
        }
        index.require_source(engine.state())?;
        let parent = engine.admitted_snapshot_root_context(repository)?;
        let branch = parent.source.branch_id;
        let namespace = NamespaceHostAddress { repository_id:repository, branch_id:branch,
            directory:NamespaceDirectoryId::Repository(repository) };
        let root = engine.namespace_host_root(repository, branch).map_err(RegistrationError::Publication)?;
        let outer = match index.discover_root(SnapshotHeadRequest {
            namespace,host:parent.host,branch:SnapshotBranchSelection::Initial,
        }, root.binding())? {
            SnapshotHeadDiscovery::Head { snapshot, .. } => snapshot,
            SnapshotHeadDiscovery::Pending { operation } => return Err(RegistrationError::Pending(operation)),
            SnapshotHeadDiscovery::NotInitialized => return Err(RegistrationError::Source),
        };
        // An indexed but unavailable parent does not certify this child host.
        index.reopen_node(engine, outer)?;
        let tree = engine.namespace_snapshot(&branch.to_string()).map_err(RegistrationError::Publication)?;
        let row = tree.tree().get_key(child.key()).filter(|row|
            row.kind == NamespaceEntryKind::Directory && row.path.parent().is_none() && ordinary_path(&row.path)
        ).ok_or(RegistrationError::Source)?;
        let entry = row.entry_id.parse::<EntryId>().map_err(RegistrationError::Allocation)?;
        let ordinary = engine.state().namespace_directories.get(&row.entry_id).ok_or(RegistrationError::Host)?;
        let disk = root.disk().disk_id.parse().map_err(RegistrationError::Allocation)?;
        if ordinary.entry != entry || ordinary.repository != repository || ordinary.branch != branch
            || ordinary.parent != NamespaceDirectoryParent::Root || ordinary.owner_disk != disk
            || ordinary.name.as_str() != row.path.as_str()
        {
            return Err(RegistrationError::Host);
        }
        let registered = index.directories();
        let directory = match registered.get(parent.host.root_id, repository, entry) {
            Some(super::DirectoryRegistration::Published(binding)) => binding.clone(),
            Some(super::DirectoryRegistration::Pending(op)) => return Err(RegistrationError::Pending(op)),
            Some(super::DirectoryRegistration::Conflict) | None => return Err(RegistrationError::Host),
        };
        let owned = crate::engine::fresh_layout::directories::owner_binding(engine.state(), ordinary)
            .map_err(RegistrationError::Publication)?;
        if owned != directory.directory || owned.parent_directory_id != parent.host.directory_id
            || owned.root_id != parent.host.root_id || owned.lifecycle != storage_contract::working_tree::BindingLifecycle::Current
        {
            return Err(RegistrationError::Host);
        }
        crate::container_storage::verify_ordinary_directory(
            engine.config(), &engine.disk_resources, engine.state(), entry, disk,
        ).map_err(RegistrationError::Publication)?;
        let common = root.common().map_err(RegistrationError::Publication)?;
        common.private_entry_matching(&crate::persist::layout::namespace::directory(&owned.name), file_identity(owned.entity))
            .map_err(|e| RegistrationError::NamespaceHost(NamespaceHostError::Physical(e)))?;
        let mut context = parent;
        context.host = SnapshotHost { directory_id:owned.directory_id, ..context.host };
        context.source.root = Some(LookupPath::from(&row.path));
        Ok(Self { context,directory })
    }
}

/// A bounded, component-delimited range of an already admitted namespace.
/// No FILE outside the registered child contributes a lazy branch or payload.
pub(crate) struct Window {
    key: String,
    depth: usize,
}
impl Window {
    pub(crate) fn new(snapshot: &NamespaceSnapshot, host: &HostEvidence,
        root: Option<&LookupPath>) -> std::result::Result<Self, NamespaceHostError> {
        match (root, host.namespace.directory, host.directories.as_slice()) {
            (None,NamespaceDirectoryId::Repository(repository),[]) if repository == host.namespace.repository_id =>
                Ok(Self { key:String::new(), depth:0 }),
            (Some(path),NamespaceDirectoryId::Entry(id),[directory]) if id == directory.entry => {
                let row = snapshot.tree().get_key(path.key()).filter(|entry|
                    entry.kind == NamespaceEntryKind::Directory && entry.entry_id == id.to_string()
                        && entry.path.parent().is_none() && entry.path.as_str() == directory.directory.name.as_str()
                        && ordinary_path(&entry.path)).ok_or(NamespaceHostError::Directory)?;
                Ok(Self { key:row.lookup_key().to_owned(), depth:1 })
            }
            _ => Err(NamespaceHostError::Directory),
        }
    }
    pub(crate) fn rows<'a>(&'a self, snapshot: &'a NamespaceSnapshot)
        -> (usize, impl Iterator<Item=(&'a crate::Entry, PortablePath)> + 'a) {
        let depth = self.depth;
        let (count, rows) = snapshot.tree().selected_descendants(&self.key);
        (count, rows.map(move |row| {
            let relative = row.path.strip_components(depth)
                .expect("invariant: selected namespace row has its complete relative suffix");
            (row,relative)
        }))
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_worktree/direct_child.rs"]
mod tests;

/// Held *before* the native reservation. Use each original descriptor again at
/// the copy, never a fallback replica, a later HEAD or a same-byte replacement.
pub(super) struct InitialBodies {
    registration: storage_contract::working_tree::DiskRegistrationIdentity,
    files: std::collections::BTreeMap<storage_contract::VersionId, crate::engine::HeldDirectoryFileBody>,
}
impl InitialBodies {
    pub(super) fn capture(engine: &Engine, host: &HostEvidence,
        manifest: &crate::working_tree::snapshot::AdmittedSnapshotManifest) -> Result<Option<Self>> {
        if host.directories.is_empty() { return Ok(None); }
        let registration = host.registration.clone();
        let disk = engine.config().disks.iter().find(|d| d.disk_id == registration.disk_id
            && d.slot_id == registration.slot_id && d.binding_generation == registration.binding_generation)
            .ok_or(RegistrationError::Host)?;
        engine.planning_context().require_first_directory_space(disk,manifest).map_err(RegistrationError::Publication)?;
        let mut files = std::collections::BTreeMap::new();
        for file in manifest.files().values() {
            let version = engine.state().versions.get(&file.version_id.to_string()).ok_or(RegistrationError::Source)?;
            if version.content_hash != file.content_hash || version.logical_size != file.size
                || engine.state().file_containers.get(&file.file_id).is_none_or(|r| r.owner_disk.to_string() != disk.disk_id)
            { return Err(RegistrationError::Source); }
            files.insert(file.version_id,engine.open_directory_file_body(file.file_id,file.version_id,disk)
                .map_err(RegistrationError::Publication)?);
        }
        Ok(Some(Self { registration,files }))
    }

    pub(super) fn require_ready(&self, engine:&Engine, manifest:&crate::working_tree::snapshot::AdmittedSnapshotManifest)
        -> Result<()> {
        let disk = engine.disk(&self.registration.slot_id).map_err(RegistrationError::Publication)?;
        if disk.disk_id != self.registration.disk_id || disk.binding_generation != self.registration.binding_generation {
            return Err(RegistrationError::Host);
        }
        engine.planning_context().require_first_directory_space(disk,manifest).map_err(RegistrationError::Publication)?;
        self.require_named()
    }

    pub(super) fn require_named(&self) -> Result<()> {
        for body in self.files.values() { body.require_named().map_err(RegistrationError::Publication)?; }
        Ok(())
    }

    pub(super) fn open(&self, file:&crate::working_tree::snapshot::SnapshotFile)
        -> std::result::Result<crate::ReadFile,crate::working_tree::snapshot::SnapshotStoreError> {
        use crate::working_tree::snapshot::SnapshotStoreError;
        self.files.get(&file.version_id).ok_or(SnapshotStoreError::Version)?
            .open_original().map_err(SnapshotStoreError::Disk)
    }
}
