//! Held automatic source directories. Geometry never adopts an existing entry:
//! only a published entity or this decision's exact preparation can be opened.
use super::*;
use crate::engine::fresh_layout::namespace::PreparedBackupSourcePath;
use platform_io::fs_safety::EntryName;
use storage_contract::file_container::ContainerPrefix;

pub(super) enum HeldParent {
    Common(Arc<DiskHandle>),
    Prefix(Arc<HeldPrefix>),
}
impl Clone for HeldParent {
    fn clone(&self) -> Self {
        match self {
            Self::Common(disk) => Self::Common(Arc::clone(disk)),
            Self::Prefix(prefix) => Self::Prefix(Arc::clone(prefix)),
        }
    }
}
impl HeldParent {
    pub(super) fn root(&self) -> Result<&DiskRoot> {
        match self {
            Self::Common(disk) => disk.content_root(),
            Self::Prefix(prefix) => {
                prefix.verify()?;
                Ok(&prefix.held)
            }
        }
    }

    pub(super) fn installed(&self) -> bool {
        match self {
            Self::Common(_) => true,
            Self::Prefix(prefix) => prefix.installed.get().is_some(),
        }
    }

    pub(super) fn published_path(&self) -> Result<PathBuf> {
        match self {
            Self::Common(disk) => Ok(disk.content_root()?.path().to_owned()),
            Self::Prefix(prefix) => Ok(prefix.parent.published_path()?
                .join(prefix.component.as_path())),
        }
    }

    pub(super) fn install(&self) -> Result<()> {
        match self {
            Self::Common(_) => Ok(()),
            Self::Prefix(prefix) => prefix.install(),
        }
    }
}

pub(super) struct HeldPrefix {
    disk: Arc<DiskHandle>,
    parent: HeldParent,
    component: EntryName,
    record: ContainerPrefix,
    held: Arc<DiskRoot>,
    entity: ContainerEntity,
    installed: OnceLock<()>,
}
impl HeldPrefix {
    fn identity(&self) -> FileIdentity {
        FileIdentity { device: self.entity.device(), inode: self.entity.inode() }
    }

    fn stage(&self) -> Result<PathBuf> {
        Ok(self.disk.management_root()?.path()
            .join(layout::container::prefix_preparation(self.record.preparation).as_path()))
    }

    pub(super) fn verify(&self) -> Result<()> {
        let parent = self.parent.root()?;
        let (root, path) = if self.installed.get().is_some() {
            if !self.parent.installed() {
                return Err(Failure::ContainerIdentity.into());
            }
            (parent, parent.path().join(self.component.as_path()))
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

    fn install(&self) -> Result<()> {
        self.verify()?;
        self.parent.install()?;
        let source = self.disk.management_root()?;
        let target = self.parent.root()?;
        if self.installed.get().is_none() {
            let to = target.path().join(self.component.as_path());
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

pub(super) type HeldPrefixes = BTreeMap<(DiskId, DiskId), Arc<HeldPrefix>>;

#[derive(Clone, Copy)]
pub(super) enum Observation<'a> {
    Prepare,
    Candidate(&'a State),
    Published,
}

fn previously_published(state: &State, disk: DiskId, expected: &ContainerPrefix) -> bool {
    state.file_containers.values().any(|record| {
        record.replicas.get(&disk).is_some_and(|replica| replica.prefixes.contains(expected))
    })
}

/// Called only after the owning publication has budgeted the complete metadata
/// shape and the shared descriptor reservation. All constructors stay here.
pub(super) fn bind_chain(
    source: &PreparedBackupSourcePath<'_>,
    disk: &Arc<DiskHandle>,
    records: &[ContainerPrefix],
    mode: Observation<'_>,
    held: &mut HeldPrefixes,
) -> Result<HeldParent> {
    let id = disk.seal.disk_id.parse::<DiskId>()?;
    if source.target_registration().disk_id != disk.seal.disk_id
        || records.iter().map(|record| record.source_disk).collect::<Vec<_>>()
            != source.destination().source_ids().collect::<Vec<_>>()
    {
        return Err(Failure::ContainerScope.into());
    }
    if !std::ptr::eq(source.target_common()?, disk.content_root()?) {
        return Err(Failure::ContainerScope.into());
    }
    let mut parent = HeldParent::Common(Arc::clone(disk));
    for (record, component) in records.iter()
        .zip(layout::container::source_components(source.destination()))
    {
        let key = (id, record.source_disk);
        if let Some(existing) = held.get(&key) {
            if existing.record != *record {
                return Err(Failure::ContainerIdentity.into());
            }
            existing.verify()?;
            parent = HeldParent::Prefix(Arc::clone(existing));
            continue;
        }
        let target = parent.root()?;
        let named = target.path().join(component.as_path());
        let management = disk.management_root()?;
        let leaf = layout::container::prefix_preparation(record.preparation);
        let stage = management.path().join(leaf.as_path());
        let installed = OnceLock::new();
        let (directory, identity) = match mode {
            Observation::Prepare if record.entity == ContainerEntity::new(0, 0) => {
                if target.optional_metadata(&named)?.is_some() {
                    return Err(Failure::ContainerIdentity.into());
                }
                create_exclusive(management, &leaf)?
            }
            Observation::Prepare | Observation::Published => {
                if !parent.installed() {
                    return Err(Failure::ContainerIdentity.into());
                }
                let identity = FileIdentity {
                    device: record.entity.device(), inode: record.entity.inode(),
                };
                let directory = target.private_child_matching(&named, identity)?;
                let _ = installed.set(());
                (directory, identity)
            }
            Observation::Candidate(previous) => {
                let identity = FileIdentity {
                    device: record.entity.device(), inode: record.entity.inode(),
                };
                if identity.inode == 0 {
                    return Err(Failure::ContainerIdentity.into());
                }
                if previously_published(previous, id, record) {
                    if !parent.installed() {
                        return Err(Failure::ContainerIdentity.into());
                    }
                    let directory = target.private_child_matching(&named, identity)?;
                    let _ = installed.set(());
                    (directory, identity)
                } else {
                    let at_name = target.optional_metadata(&named)?.is_some();
                    let at_stage = management.optional_metadata(&stage)?.is_some();
                    match (at_name, at_stage, parent.installed()) {
                        (false, true, _) => {
                            (management.private_child_matching(&stage, identity)?, identity)
                        }
                        (true, false, true) => {
                            let directory = target.private_child_matching(&named, identity)?;
                            let _ = installed.set(());
                            (directory, identity)
                        }
                        _ => return Err(Failure::ContainerIdentity.into()),
                    }
                }
            }
        };
        let prefix = Arc::new(HeldPrefix {
            disk: Arc::clone(disk), parent, component, record: record.clone(),
            held: Arc::new(directory),
            entity: ContainerEntity::new(identity.device, identity.inode), installed,
        });
        prefix.verify()?;
        parent = HeldParent::Prefix(Arc::clone(&prefix));
        held.insert(key, prefix);
    }
    Ok(parent)
}

pub(super) fn entity(held: &HeldPrefixes, disk: DiskId, source: DiskId) -> Result<ContainerEntity> {
    let prefix = held.get(&(disk, source)).ok_or(Failure::ContainerScope)?;
    prefix.verify()?;
    Ok(prefix.entity)
}

pub(super) fn create_exclusive(root: &DiskRoot, leaf: &EntryName)
    -> Result<(DiskRoot, FileIdentity)>
{
    match root.create_private_entry(leaf) {
        DirectoryCreateOutcome::Created { directory, identity, durability } => {
            durability.map_err(crate::engine::fresh_layout::directory_error)?;
            root.sync_private_entry_matching(leaf, identity)
                .map_err(crate::engine::fresh_layout::directory_error)?;
            Ok((directory, identity))
        }
        DirectoryCreateOutcome::Rejected(error)
        | DirectoryCreateOutcome::Uncertain { source: error, .. } => {
            Err(crate::engine::fresh_layout::directory_error(error))
        }
    }
}

/// Final-only reader. Each name must carry the exact prefix entity published in
/// this replica; matching BODYs or a configured name never identify a prefix.
pub(crate) fn open_published_backup_parent(
    source: &PreparedBackupSourcePath<'_>,
    records: &[ContainerPrefix],
) -> Result<DiskRoot> {
    if records.is_empty()
        || records.iter().map(|record| record.source_disk).collect::<Vec<_>>()
            != source.destination().source_ids().collect::<Vec<_>>()
    {
        return Err(Failure::ContainerScope.into());
    }
    let mut ancestors = Vec::with_capacity(records.len());
    for (record, component) in records.iter()
        .zip(layout::container::source_components(source.destination()))
    {
        let parent = match ancestors.last() {
            Some(directory) => directory,
            None => source.target_common()?,
        };
        let identity = FileIdentity {
            device: record.entity.device(), inode: record.entity.inode(),
        };
        if identity.inode == 0 {
            return Err(Failure::ContainerIdentity.into());
        }
        let next = parent.private_child_matching(
            &parent.path().join(component.as_path()), identity,
        )?;
        ancestors.push(next);
    }
    // Opening the inner A descriptor must not hide replacement of its outer B
    // name while admission was in progress. Recheck each name through the same
    // retained parent, then return the already observed final descriptor.
    for (index, (record, component)) in records.iter()
        .zip(layout::container::source_components(source.destination())).enumerate()
    {
        let parent = if index == 0 { source.target_common()? } else { &ancestors[index - 1] };
        let observed = parent.private_child_matching(
            &parent.path().join(component.as_path()),
            FileIdentity { device: record.entity.device(), inode: record.entity.inode() },
        )?;
        let retained = &ancestors[index];
        if observed.directory_identity(observed.path())?
            != retained.directory_identity(retained.path())?
        {
            return Err(Failure::ContainerIdentity.into());
        }
    }
    source.target_common()?;
    ancestors.pop().ok_or_else(|| Failure::ContainerScope.into())
}
