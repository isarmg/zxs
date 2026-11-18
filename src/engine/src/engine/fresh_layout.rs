//! Fresh-only physical root binding. A caller's Config is not an observed layout.
//! Creation runs under the initializer's existing directory and writer fences;
//! the resulting bindings enter the fixed genesis configuration before I/O binds.
use crate::{Config, DiskConfig, ErrorCode, Result, ValidatedConfig};
use crate::disk::{DiskResources, DiskRoot};
use crate::persist::layout;
use crate::namespace_roots::{self, file_identity, local_entity};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{DiskId, Generation, OperationId};
use storage_contract::working_tree::{
    DirectoryId, DiskRegistrationIdentity, RootBinding, RootId,
    StorageLayout,
};

pub(in crate::engine) mod root_areas;
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
    #[error("the fresh Add request must preserve its exact admitted original configuration")]
    AdditionRequest,
    #[error("fresh root preparation admits one new device per Add")]
    AdditionCount,
    #[error("the Add operation, source epoch, actor or exact request differs")]
    AdditionConflict,
    #[error("original members disagree on the immutable Add layout preparation")]
    AdditionFork,
    #[error("fresh root preparation requires clean original journals")]
    AdditionJournals,
    #[error("membership fence sequence exhausted: {0}")]
    AdditionSequence(#[source] storage_contract::SequenceExhausted),
    #[error("the Add actor is empty: {0}")]
    AdditionPrincipal(#[source] storage_contract::EmptyPrincipal),
    #[error("an unrecorded Add preparation exists; its directory cannot be adopted")]
    AdditionUnrecorded,
    #[error("the Add layout feedback differs from its request or held physical roots")]
    AdditionEvidence,
    #[error("decoding the immutable Add layout preparation: {0}")]
    AdditionRecord(#[source] storage_contract::Error),
    #[error("the Backup root feedback differs from its exact frozen source or target set")]
    BackupRootEvidence,
    #[error("the retained Backup target ownership cannot be observed: {0}")]
    BackupRootOwnership(#[source] std::io::Error),
    #[error("original members disagree on the immutable Backup root preparation")]
    BackupRootFork,
    #[error("the Backup root preparation is missing or incomplete on original members")]
    BackupRootMissing,
    #[error("unrecorded Backup lifecycle roots cannot be adopted")]
    BackupRootUnrecorded,
    #[error("decoding the immutable Backup root preparation: {0}")]
    BackupRootRecord(#[source] storage_contract::Error),
    #[error("Backup root feedback exceeds its bounded metadata reservation")]
    BackupRootBudget,
    #[error("a selected Backup target or original member cannot retain root feedback")]
    BackupRootSpace,
    #[error("Backup target namespace feedback differs from its complete source")]
    BackupLayoutEvidence,
    #[error("Backup target layout requires ordinary current automatic file-manager units")]
    BackupLayoutUnsupported,
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
    #[error("complete physical preparation bytes exceed the bounded size domain")]
    PhysicalBytesOverflow,
    #[error("disk {disk} cannot admit the complete physical preparation and metadata tail")]
    PhysicalCapacity { disk: DiskId },
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
            Self::AdditionRequest => ErrorCode::ConfigConflict,
            Self::AdditionCount => ErrorCode::UnsupportedOperation,
            Self::AdditionConflict => ErrorCode::IdempotencyKeyReused,
            Self::AdditionFork => ErrorCode::LogFork,
            Self::AdditionJournals | Self::AdditionSequence(_) => ErrorCode::RecoveryRequired,
            Self::AdditionPrincipal(_) => ErrorCode::Unauthenticated,
            Self::AdditionUnrecorded => ErrorCode::RecoveryRequired,
            Self::AdditionEvidence => ErrorCode::DiskIdentityMismatch,
            Self::AdditionRecord(_) => ErrorCode::ObjectCorrupt,
            Self::BackupRootEvidence => ErrorCode::DiskIdentityMismatch,
            Self::BackupLayoutEvidence => ErrorCode::ContainerLayoutInvalid,
            Self::BackupLayoutUnsupported => ErrorCode::UnsupportedOperation,
            Self::BackupRootFork => ErrorCode::LogFork,
            Self::BackupRootMissing | Self::BackupRootUnrecorded | Self::BackupRootOwnership(_) => {
                ErrorCode::RecoveryRequired
            }
            Self::BackupRootRecord(_) => ErrorCode::ObjectCorrupt,
            Self::BackupRootBudget => ErrorCode::ResourceLimit,
            Self::BackupRootSpace => ErrorCode::NoSpace,
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
            Self::ContainerBudget | Self::PhysicalBytesOverflow => ErrorCode::ResourceLimit,
            Self::PhysicalCapacity { .. } => ErrorCode::NoSpace,
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
            let areas = root_areas::HeldRootAreas::create(held, directory_ids)?;
            let binding = RootBinding {
                layout: StorageLayout::UnifiedWorkingTreeV1,
                root_id,
                registration: registration(config, disk),
                source_chain: source_chain(config, disk)?,
                root_entity,
                roots: areas.binding().clone(),
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
pub(in crate::engine) fn registration(
    config: &Config, disk: &DiskConfig,
) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
pub(in crate::engine) fn source_chain(
    config: &Config, disk: &DiskConfig,
) -> Result<Vec<DiskRegistrationIdentity>> {
    let mut source = disk.upstream_slot_id.as_deref();
    let mut seen = BTreeSet::from([disk.slot_id.as_str()]);
    let mut chain = Vec::new();
    while let Some(slot) = source {
        if !seen.insert(slot) || chain.len() == namespace_roots::MAX_SOURCE_CHAIN {
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
    namespace_roots::inspect(StorageLayout::UnifiedWorkingTreeV1, config.storage_roots.values())
        .map_err(|source| FreshLayoutError::Structure(std::io::Error::other(source)))?;
    for disk in &config.disks {
        let binding = config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?)
            .ok_or(FreshLayoutError::Registration)?;
        if binding.registration != registration(config, disk)
            || binding.source_chain != source_chain(config, disk)?
        {
            return Err(FreshLayoutError::Registration.into());
        }
    }
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
