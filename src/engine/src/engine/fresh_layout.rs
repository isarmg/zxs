//! Fresh-only physical root binding. A caller's Config is not an observed layout.
//! Creation runs under the initializer's existing directory and writer fences;
//! the resulting bindings enter the fixed genesis configuration before I/O binds.
use crate::{Config, DiskConfig, ErrorCode, Result, ValidatedConfig};
use crate::disk::{DirectoryCreateOutcome, DiskResources, DiskRoot};
use crate::persist::layout;
use crate::working_tree::{
    admission,
    records::{file_identity, local_entity},
};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{DiskId, Generation, OperationId};
use storage_contract::working_tree::{
    DirectoryId, DiskRegistrationIdentity, LocalDirectoryBinding, RootBinding, RootId,
    StorageLayout, ThreeRootBinding, WorkingTreeCatalog, ZxsLifecycleBindings,
};

pub(crate) mod containers;
pub(crate) mod directories;
pub(crate) mod draft_source;
pub(crate) mod namespace;

#[derive(Debug, error_definition::Error)]
pub enum FreshLayoutError {
    #[error("fresh initialization requires an empty layout request and exact selected roots")]
    InitializationInput,
    #[error("fresh layout belongs to a different initialization occurrence")]
    InitializationSource,
    #[error("the persisted layout does not cover the exact registered disk set")]
    Registration,
    #[error("the registered physical root or one of its three roots changed")]
    RootIdentity,
    #[error("the version publication proof is outside its retained disk")]
    PublicationLocation,
    #[error("source-scoped version material requires an admitted container lifecycle source")]
    VersionSource,
    #[error("source-scoped damaged-version repair requires a held quarantine transaction")]
    SourceQuarantine,
    #[error("this namespace has no published genesis common-root registration")]
    NamespaceUnmapped,
    #[error("whole-container lifecycle transactions are not supported by this physical layout")]
    WholeContainerLifecycleUnsupported,
    #[error("the selected namespace does not match its exact root creation source")]
    NamespaceSource,
    #[error("the permanent namespace root creation record is invalid: {0}")]
    NamespaceRecord(#[source] json_codec::Error),
    #[error("native container records do not close the exact file inventory")]
    ContainerShape,
    #[error("container preparation does not preserve its actual input scope")]
    ContainerScope,
    #[error("the physical container differs from its published entity")]
    ContainerIdentity,
    #[error("the source slice's retained area changed: {0}")]
    ContainerArea(#[source] std::io::Error),
    #[error("completed physical metadata differs from its exact budget shape")]
    ContainerBudget,
    #[error("the verified draft body changed before publication")]
    DraftChanged,
    #[error("the draft body exceeds the configured file limit")]
    DraftTooLarge,
    #[error("the persisted layout has invalid structural bindings: {0}")]
    Structure(#[source] std::io::Error),
    #[error("the named root binding no longer identifies its original directory: {0}")]
    NamedRoot(#[source] std::io::Error),
    #[error("creating or reading the selected physical layout: {0}")]
    Directory(#[source] std::io::Error),
}
impl FreshLayoutError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::DraftChanged => ErrorCode::PreparationStale,
            Self::DraftTooLarge => ErrorCode::FileTooLarge,
            Self::InitializationInput | Self::InitializationSource => ErrorCode::ConfigInvalid,
            Self::Registration | Self::RootIdentity | Self::Structure(_) | Self::NamedRoot(_) => {
                ErrorCode::DiskIdentityMismatch
            }
            Self::Directory(_) => ErrorCode::IoError,
            Self::PublicationLocation => ErrorCode::PublicationInvalid,
            Self::VersionSource | Self::SourceQuarantine => ErrorCode::UnsupportedOperation,
            Self::ContainerShape => ErrorCode::StateInvalid,
            Self::NamespaceSource | Self::NamespaceRecord(_) => ErrorCode::PublicationInvalid,
            Self::NamespaceUnmapped | Self::WholeContainerLifecycleUnsupported => {
                ErrorCode::UnsupportedOperation
            }
            Self::ContainerScope => ErrorCode::PublicationInvalid,
            Self::ContainerIdentity | Self::ContainerArea(_) => ErrorCode::ContainerLayoutInvalid,
            Self::ContainerBudget => ErrorCode::ResourceLimit,
        }
    }
}

/// New lifecycle writes have no whole-container physical transaction yet.
pub(crate) fn require_whole_container_lifecycle() -> Result<()> {
    Err(FreshLayoutError::WholeContainerLifecycleUnsupported.into())
}

pub(super) struct CreatedRoots {
    input: ValidatedConfig,
    roots: BTreeMap<DiskId, RootBinding>,
}
impl CreatedRoots {
    pub(super) fn create(config: &ValidatedConfig, resources: &DiskResources) -> Result<Self> {
        if !config.storage_roots.is_empty() || config.generation().is_some() {
            return Err(FreshLayoutError::InitializationInput.into());
        }
        // Allocate every logical identity before the first mkdir. No filename,
        // observed inode or later Placement is used to generate an identity.
        let operation = OperationId::generate()?;
        let identities = config.disks.iter().map(|disk| {
            Ok((
                disk.disk_id.parse::<DiskId>()?,
                RootId::generate()?,
                [DirectoryId::generate()?, DirectoryId::generate()?, DirectoryId::generate()?,
                    DirectoryId::generate()?, DirectoryId::generate()?],
            ))
        }).collect::<Result<Vec<_>>>()?;
        let mut roots = BTreeMap::new();
        for (disk, (disk_id, root_id, directory_ids)) in config.disks.iter().zip(identities) {
            let held = &resources.get(disk)?.root;
            let root_entity = local_entity(held.directory_identity(held.path())?);
            let names = [
                &*layout::COMMON_DATA_ROOT, &*layout::DELETED_DATA_ROOT, &*layout::ZXS_DATA_ROOT,
            ];
            let mut created = Vec::with_capacity(3);
            for (name, directory_id) in names.into_iter().zip(directory_ids) {
                let entity = match held.create_private_entry(name) {
                    DirectoryCreateOutcome::Created { directory, identity, durability } => {
                        durability.map_err(directory_error)?;
                        // Keep the returned exact descriptor through readback;
                        // creation success is not inferred from the name alone.
                        if directory.directory_identity(directory.path())? != identity {
                            return Err(FreshLayoutError::RootIdentity.into());
                        }
                        held.sync_private_entry_matching(name, identity).map_err(directory_error)?;
                        local_entity(identity)
                    }
                    DirectoryCreateOutcome::Rejected(error)
                    | DirectoryCreateOutcome::Uncertain { source: error, .. } => {
                        return Err(directory_error(error));
                    }
                };
                created.push(LocalDirectoryBinding { directory_id, entity });
            }
            let management = held.private_entry_matching(
                &layout::ZXS_DATA_ROOT, file_identity(created[2].entity),
            ).map_err(directory_error)?;
            let lifecycle = [&*layout::COMMON_DATA_ROOT, &*layout::DELETED_DATA_ROOT]
                .into_iter().zip(directory_ids.into_iter().skip(3))
                .map(|(name, directory_id)| {
                    let (directory, identity) = match management.create_private_entry(name) {
                        DirectoryCreateOutcome::Created { directory, identity, durability } => {
                            durability.map_err(directory_error)?;
                            (directory, identity)
                        }
                        DirectoryCreateOutcome::Rejected(error)
                        | DirectoryCreateOutcome::Uncertain { source: error, .. } => {
                            return Err(directory_error(error));
                        }
                    };
                    if directory.directory_identity(directory.path())? != identity {
                        return Err(FreshLayoutError::RootIdentity.into());
                    }
                    management.sync_private_entry_matching(name, identity)
                        .map_err(directory_error)?;
                    Ok(LocalDirectoryBinding { directory_id, entity: local_entity(identity) })
                }).collect::<Result<Vec<_>>>()?;
            management.sync_dir(management.path())?;
            held.sync_dir(held.path())?;
            let mut lifecycle = lifecycle.into_iter();
            let mut created = created.into_iter();
            let binding = RootBinding {
                layout: StorageLayout::UnifiedWorkingTreeV1,
                root_id,
                registration: registration(config, disk),
                source_chain: source_chain(config, disk)?,
                root_entity,
                roots: ThreeRootBinding {
                    common_data: created.next().expect("invariant: three created roots"),
                    deleted_data: created.next().expect("invariant: three created roots"),
                    zxs_data: created.next().expect("invariant: three created roots"),
                    zxs_lifecycles: ZxsLifecycleBindings {
                        common_data: lifecycle.next().expect("invariant: two created partitions"),
                        deleted_data: lifecycle.next().expect("invariant: two created partitions"),
                    },
                },
                operation_id: operation,
                generation: Generation::new(1),
            };
            roots.insert(disk_id, binding);
        }
        let mut prospective = (**config).clone();
        prospective.storage_roots = roots.clone();
        validate_roots(&prospective)?;
        for disk in &config.disks {
            verify_bound_root(&prospective, disk, &resources.get(disk)?.root)?;
        }
        Ok(Self { input: config.clone(), roots })
    }

    pub(super) fn for_configuration(
        &self,
        config: &ValidatedConfig,
    ) -> Result<&BTreeMap<DiskId, RootBinding>> {
        if !self.input.same_admission(config) || !config.storage_roots.is_empty() {
            return Err(FreshLayoutError::InitializationSource.into());
        }
        Ok(&self.roots)
    }
}

pub(crate) fn directory_error(source: crate::disk::DirectoryIoError) -> crate::Error {
    match source {
        crate::disk::DirectoryIoError::Gate(error) => error,
        crate::disk::DirectoryIoError::Filesystem(error) => error.into(),
        crate::disk::DirectoryIoError::Io(error) => error.into(),
        changed @ (crate::disk::DirectoryIoError::WrongRoot
        | crate::disk::DirectoryIoError::IdentityChanged(_)
        | crate::disk::DirectoryIoError::NotPrivate) => {
            FreshLayoutError::NamedRoot(std::io::Error::other(changed)).into()
        }
        source @ (crate::disk::DirectoryIoError::ProtectedEntry(_)
        | crate::disk::DirectoryIoError::Collision(_)
        | crate::disk::DirectoryIoError::WrongFilesystem
        | crate::disk::DirectoryIoError::InventoryBudget
        | crate::disk::DirectoryIoError::Sync { .. }
        | crate::disk::DirectoryIoError::Tail { .. }) => {
            FreshLayoutError::Directory(std::io::Error::other(source)).into()
        }
    }
}
fn registration(config: &Config, disk: &DiskConfig) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
fn source_chain(config: &Config, disk: &DiskConfig) -> Result<Vec<DiskRegistrationIdentity>> {
    let mut source = disk.upstream_slot_id.as_deref();
    let mut seen = BTreeSet::from([disk.slot_id.as_str()]);
    let mut chain = Vec::new();
    while let Some(slot) = source {
        if !seen.insert(slot) || chain.len() == admission::MAX_SOURCE_CHAIN {
            return Err(FreshLayoutError::Registration.into());
        }
        let parent = config.disks.iter().find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
            .ok_or(FreshLayoutError::Registration)?;
        chain.push(registration(config, parent));
        source = parent.upstream_slot_id.as_deref();
    }
    Ok(chain)
}

/// Shape checks do not authenticate Config. Empty roots are accepted only as
/// the public pre-initialization request; every cold physical open rejects them.
pub(super) fn validate_roots(config: &Config) -> Result<()> {
    if config.storage_roots.is_empty() {
        return Ok(());
    }
    if config.storage_roots.len() != config.disks.len() {
        return Err(FreshLayoutError::Registration.into());
    }
    for disk in &config.disks {
        let binding = config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?)
            .ok_or(FreshLayoutError::Registration)?;
        if binding.registration != registration(config, disk)
            || binding.source_chain != source_chain(config, disk)?
        {
            return Err(FreshLayoutError::Registration.into());
        }
    }
    let mut roots: Vec<_> = config.storage_roots.values().cloned().collect();
    roots.sort_by_key(|binding| binding.root_id);
    let catalog = WorkingTreeCatalog {
        storage_layout: StorageLayout::UnifiedWorkingTreeV1,
        roots, directories: Vec::new(), containers: Vec::new(),
        snapshot_repositories: Vec::new(), replicas: Vec::new(),
    };
    admission::validate_catalog_structure(&catalog)
        .map_err(|source| FreshLayoutError::Structure(std::io::Error::other(source)))?;
    Ok(())
}

pub(crate) fn verify_root(
    config: &ValidatedConfig,
    disk: &DiskConfig,
    root: &DiskRoot,
) -> Result<()> {
    verify_bound_root(config, disk, root)
}
fn verify_bound_root(config: &Config, disk: &DiskConfig, root: &DiskRoot) -> Result<()> {
    let binding = config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?)
        .ok_or(FreshLayoutError::Registration)?;
    // An open descriptor can outlive the configured name. Observe that name
    // under the original I/O lease before trusting any child or seal through
    // the held root; a moved disk is not an available source or destination.
    if binding.registration != registration(config, disk)
        || super::membership_transition::verify_named_root(root)?
            != file_identity(binding.root_entity)
    {
        return Err(FreshLayoutError::RootIdentity.into());
    }
    for (name, directory) in [
        (&*layout::COMMON_DATA_ROOT, &binding.roots.common_data),
        (&*layout::DELETED_DATA_ROOT, &binding.roots.deleted_data),
        (&*layout::ZXS_DATA_ROOT, &binding.roots.zxs_data),
    ] {
        let held = root.private_entry_matching(name, file_identity(directory.entity))
            .map_err(directory_error)?;
        if held.directory_identity(held.path())? != file_identity(directory.entity) {
            return Err(FreshLayoutError::RootIdentity.into());
        }
    }
    let management = root.private_entry_matching(
        &layout::ZXS_DATA_ROOT, file_identity(binding.roots.zxs_data.entity),
    ).map_err(directory_error)?;
    for (name, directory) in [
        (&*layout::COMMON_DATA_ROOT, &binding.roots.zxs_lifecycles.common_data),
        (&*layout::DELETED_DATA_ROOT, &binding.roots.zxs_lifecycles.deleted_data),
    ] {
        management.private_entry_matching(name, file_identity(directory.entity))
            .map_err(directory_error)?;
    }
    Ok(())
}

/// Revalidation, not a material or layout mutation grant. Excluded/unavailable
/// disks are not probed; every held registered root keeps its exact named roots.
pub(crate) fn verify_available(config: &ValidatedConfig, resources: &DiskResources) -> Result<()> {
    for disk in config.io_disks() {
        if let Some(held) = resources.available(disk) {
            verify_root(config, disk, &held.root)?;
        }
    }
    Ok(())
}
