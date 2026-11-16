//! Held ordinary parents. The only creator is the already budgeted ordinary
//! publication; reads require an exact published or same-Candidate occurrence.
use super::*;
use storage_contract::EntryId;
use storage_contract::file_container::{
    NamespaceDirectoryParent as Parent, NamespaceDirectoryRecord as Record,
    NamespaceDirectoryReplica as Replica,
};

pub(super) type HeldDirectories = BTreeMap<(EntryId, DiskId), Arc<HeldDirectory>>;

pub(super) struct HeldDirectory {
    disk: Arc<DiskHandle>,
    parent: HeldParent,
    name: name_policy::LookupName,
    record: Replica,
    held: DiskRoot,
    installed: OnceLock<()>,
    _reservation: Arc<crate::engine::PayloadProofReservation>,
}
impl HeldDirectory {
    fn identity(&self) -> FileIdentity {
        FileIdentity { device: self.record.entity.device(), inode: self.record.entity.inode() }
    }

    fn stage(&self) -> Result<PathBuf> {
        Ok(self.disk.management_root()?.path()
            .join(layout::container::namespace_preparation(self.record.preparation).as_path()))
    }

    pub(super) fn verify(&self) -> Result<()> {
        let parent = self.parent.root()?;
        let identity = parent.directory_identity(parent.path())?;
        if ContainerEntity::new(identity.device, identity.inode) != self.record.parent_entity {
            return Err(Failure::ContainerIdentity.into());
        }
        let (root, path) = if self.installed.get().is_some() {
            if !self.parent.installed() {
                return Err(Failure::ContainerIdentity.into());
            }
            (parent, parent.path().join(layout::namespace::directory(&self.name).as_path()))
        } else {
            (self.disk.management_root()?, self.stage()?)
        };
        let observed = root.private_child_matching(&path, self.identity())?;
        if observed.directory_identity(observed.path())?
            != self.held.directory_identity(self.held.path())?
        {
            return Err(Failure::ContainerIdentity.into());
        }
        Ok(())
    }

    pub(super) fn root(&self) -> Result<&DiskRoot> {
        self.verify()?;
        Ok(&self.held)
    }

    pub(super) fn installed(&self) -> bool {
        self.installed.get().is_some()
    }
    pub(super) fn current_zxs(&self) -> Result<&DiskRoot> {
        self.verify()?;
        self.parent.current_zxs()
    }
    pub(super) fn published_path(&self) -> Result<PathBuf> {
        Ok(self.parent.published_path()?.join(layout::namespace::directory(&self.name).as_path()))
    }
    pub(super) fn record(&self) -> Result<Replica> {
        self.verify()?;
        Ok(self.record.clone())
    }

    pub(super) fn install(&self) -> Result<()> {
        self.verify()?;
        self.parent.install()?;
        let source = self.disk.management_root()?;
        let target = self.parent.root()?;
        if self.installed.get().is_none() {
            let to = target.path().join(layout::namespace::directory(&self.name).as_path());
            if target.optional_metadata(&to)?.is_some() {
                return Err(Failure::ContainerIdentity.into());
            }
            source.rename_to_held_synced(&self.stage()?, target, &to)?;
            target.private_child_matching(&to, self.identity())?;
            self.installed.set(()).map_err(|_| Failure::ContainerIdentity)?;
        } else {
            target.sync_dir(target.path())?;
            source.sync_dir(source.path())?;
        }
        self.verify()
    }
}

/// Target selection is a pure union of the same FILE targets and actual Mkdir
/// additions. It cannot turn an unavailable target into an absent directory.
pub(super) fn targets(
    state: &State, files: Option<&BTreeSet<(FileId, DiskId)>>,
    additions: Option<&BTreeSet<(EntryId, DiskId)>>,
) -> Result<BTreeSet<(EntryId, DiskId)>> {
    let mut selected = BTreeSet::new();
    if files.is_none() {
        for record in state.namespace_directories.values() {
            selected.extend(record.replicas.keys().map(|disk| (record.entry, *disk)));
        }
    } else {
        for (file, disk) in files.into_iter().flatten() {
            let record = state.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            if let Parent::Directory(entry) = record.namespace.parent {
                for parent in crate::engine::fresh_layout::directories::chain(state, entry)? {
                    selected.insert((parent.entry, *disk));
                }
            }
        }
    }
    for (entry, disk) in additions.into_iter().flatten() {
        for parent in crate::engine::fresh_layout::directories::chain(state, *entry)? {
            selected.insert((parent.entry, *disk));
            let replica = parent.replicas.get(disk).ok_or(Failure::NamespaceUnmapped)?;
            for source in &replica.source_chain {
                let source = source.disk_id.parse::<DiskId>()?;
                if !parent.replicas.contains_key(&source) {
                    return Err(Failure::NamespaceSource.into());
                }
                selected.insert((parent.entry, source));
            }
        }
    }
    Ok(selected)
}

/// Observe the same EntryId at the direct predecessor before preparing a new
/// downstream occurrence. DiskId lexical order is not route order.
pub(super) fn ordered(
    state: &State, targets: BTreeSet<(EntryId, DiskId)>,
) -> Result<Vec<(EntryId, DiskId)>> {
    let mut ordered = Vec::with_capacity(targets.len());
    for (entry, disk) in targets {
        let record = state.namespace_directories.get(&entry.to_string())
            .ok_or(Failure::NamespaceUnmapped)?;
        let replica = record.replicas.get(&disk).ok_or(Failure::NamespaceUnmapped)?;
        ordered.push((replica.source_chain.len(), entry, disk));
    }
    ordered.sort();
    Ok(ordered.into_iter().map(|(_, entry, disk)| (entry, disk)).collect())
}

/// Check all newly introduced components before any EXCL creation. Name limits
/// come from the actual held destination filesystem, never a path-derived root.
pub(super) fn preflight(
    config: &Config, resources: &DiskResources, previous: &State, state: &State,
    targets: &BTreeSet<(EntryId, DiskId)>,
) -> Result<()> {
    for (entry, disk) in targets {
        let record = state.namespace_directories.get(&entry.to_string())
            .ok_or(Failure::NamespaceUnmapped)?;
        let target = if *disk == record.owner_disk {
            let source = crate::engine::fresh_layout::directories::read_owner(
                config, resources, previous, record, *disk,
            )?;
            source.common()?;
            resources.get(configured_disk(config, *disk)?)?.content_root()?
        } else {
            let source = crate::engine::fresh_layout::directories::read_backup(
                config, resources, previous, record, *disk,
            )?;
            let replica = record.replicas.get(disk).ok_or(Failure::NamespaceUnmapped)?;
            if replica.source_prefixes.iter().any(|prefix| prefix.entity.inode() == 0) {
                source.verify_new_component_budget()?;
                for name in [
                    &*layout::COMMON_DATA_ROOT, &*layout::DELETED_DATA_ROOT,
                    &*layout::ZXS_DATA_ROOT,
                ] {
                    version_names::check_target_name(source.target_root()?, name)?;
                }
                let handle = resources.get(configured_disk(config, *disk)?)?;
                for prefix in &replica.source_prefixes {
                    version_names::check_target_name(handle.management_root()?,
                        &layout::container::prefix_preparation(prefix.preparation))?;
                }
            }
            &resources.get(configured_disk(config, *disk)?)?.root
        };
        let replica = record.replicas.get(disk).ok_or(Failure::NamespaceUnmapped)?;
        if replica.entity.inode() == 0 {
            version_names::check_target_name(target,
                &layout::namespace::directory(&record.name))?;
            let handle = resources.get(configured_disk(config, *disk)?)?;
            version_names::check_target_name(handle.management_root()?,
                &layout::container::namespace_preparation(replica.preparation))?;
        }
    }
    Ok(())
}

pub(super) fn bind(
    config: &Config, resources: &DiskResources, source_state: &State, state: &State,
    entry: EntryId, disk: DiskId, mode: Observation<'_>,
    reservation: &Arc<crate::engine::PayloadProofReservation>, held: &mut HeldDirectories,
    prefixes: &mut HeldPrefixes,
) -> Result<HeldParent> {
    let chain = crate::engine::fresh_layout::directories::chain(state, entry)?;
    let first = *chain.first().ok_or(Failure::NamespaceUnmapped)?;
    let leaf = *chain.last().ok_or(Failure::NamespaceUnmapped)?;
    let occurrence = leaf.replicas.get(&disk).ok_or(Failure::NamespaceUnmapped)?;
    if matches!(mode, Observation::Prepare) && occurrence.entity.inode() == 0
        && let Some(direct) = occurrence.source_chain.first()
    {
        held.get(&(entry, direct.disk_id.parse::<DiskId>()?))
            .ok_or(Failure::NamespaceSource)?.root()?;
    }
    let handle = resources.get(configured_disk(config, disk)?)?;
    let parent = if disk == first.owner_disk {
        let source = crate::engine::fresh_layout::directories::read_owner(
            config, resources, source_state, first, disk,
        )?;
        if !std::ptr::eq(source.common()?, handle.content_root()?) {
            return Err(Failure::NamespaceSource.into());
        }
        HeldParent::Common(Arc::clone(handle))
    } else {
        let source = crate::engine::fresh_layout::directories::read_backup(
            config, resources, source_state, first, disk,
        )?;
        source.require_source(source_state, config, resources)?;
        prefix::bind_chain(&source, handle, &first.replicas[&disk].source_prefixes, mode, prefixes)?
    };
    attach(config, state, entry, disk, handle, parent, mode, reservation, held)
}

/// The caller already holds the exact FILE source slice. Reuse that parent and
/// its descriptors; do not reopen another prefix chain outside the reservation.
pub(super) fn attach(
    config: &Config, state: &State, entry: EntryId, disk: DiskId, handle: &Arc<DiskHandle>,
    mut parent: HeldParent, mode: Observation<'_>,
    reservation: &Arc<crate::engine::PayloadProofReservation>, held: &mut HeldDirectories,
) -> Result<HeldParent> {
    let chain = crate::engine::fresh_layout::directories::chain(state, entry)?;
    let first = *chain.first().ok_or(Failure::NamespaceUnmapped)?;
    let binding = config.storage_roots.get(&disk).ok_or(Failure::Registration)?;
    for record in chain {
        let replica = record.replicas.get(&disk).ok_or(Failure::NamespaceUnmapped)?;
        if record.repository != first.repository || record.branch != first.branch
            || record.owner_disk != first.owner_disk || replica.root_id != binding.root_id
            || (disk == record.owner_disk && !replica.source_chain.is_empty())
            || (disk != record.owner_disk && replica.source_chain != binding.source_chain)
        {
            return Err(Failure::NamespaceSource.into());
        }
        let key = (record.entry, disk);
        if let Some(observed) = held.get(&key) {
            observed.verify()?;
            if observed.name != record.name
                || (record.replicas[&disk].entity.inode() != 0
                    && observed.record != record.replicas[&disk])
            {
                return Err(Failure::ContainerIdentity.into());
            }
            parent = HeldParent::Directory(Arc::clone(observed));
            continue;
        }
        let observed = capture(handle, parent, record, disk, mode, reservation)?;
        let observed = Arc::new(observed);
        parent = HeldParent::Directory(Arc::clone(&observed));
        held.insert(key, observed);
    }
    Ok(parent)
}

fn capture(
    disk: &Arc<DiskHandle>, parent: HeldParent, logical: &Record, id: DiskId,
    mode: Observation<'_>, reservation: &Arc<crate::engine::PayloadProofReservation>,
) -> Result<HeldDirectory> {
    let mut record = logical.replicas.get(&id).ok_or(Failure::NamespaceUnmapped)?.clone();
    let target = parent.root()?;
    let parent_identity = target.directory_identity(target.path())?;
    let actual_parent = ContainerEntity::new(parent_identity.device, parent_identity.inode);
    if record.parent_entity.inode() != 0 && record.parent_entity != actual_parent {
        return Err(Failure::ContainerIdentity.into());
    }
    let named = target.path().join(layout::namespace::directory(&logical.name).as_path());
    let management = disk.management_root()?;
    let stage_name = layout::container::namespace_preparation(record.preparation);
    let stage = management.path().join(stage_name.as_path());
    let installed = OnceLock::new();
    let old = match mode {
        Observation::Prepare => record.entity.inode() != 0,
        Observation::Published => true,
        Observation::Candidate(previous) => previous.namespace_directories
            .get(&logical.entry.to_string()).is_some_and(|old| old.replicas.contains_key(&id)),
    };
    let held = if old {
        if !parent.installed() {
            return Err(Failure::ContainerIdentity.into());
        }
        let directory = open_published_container(target, &logical.name, record.entity)?;
        let _ = installed.set(());
        directory
    } else if matches!(mode, Observation::Prepare) {
        if target.optional_metadata(&named)?.is_some() {
            return Err(Failure::ContainerIdentity.into());
        }
        let (directory, identity) = prefix::create_exclusive(management, &stage_name)?;
        record.entity = ContainerEntity::new(identity.device, identity.inode);
        directory
    } else {
        let identity = FileIdentity {
            device: record.entity.device(),
            inode: record.entity.inode(),
        };
        match (target.optional_metadata(&named)?.is_some(),
            management.optional_metadata(&stage)?.is_some())
        {
            (false, true) => management.private_child_matching(&stage, identity)?,
            (true, false) if parent.installed() => {
                let directory = target.private_child_matching(&named, identity)?;
                let _ = installed.set(());
                directory
            }
            _ => return Err(Failure::ContainerIdentity.into()),
        }
    };
    record.parent_entity = actual_parent;
    let result = HeldDirectory {
        disk: Arc::clone(disk), parent, name: logical.name.clone(), record, held, installed,
        _reservation: Arc::clone(reservation),
    };
    result.verify()?;
    Ok(result)
}
