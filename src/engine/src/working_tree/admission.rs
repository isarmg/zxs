//! Bounded structural closure and explicit root registration comparison only.
use crate::DiskRegistrationIdentity;
use crate::persist::layout;
use crate::working_tree::records::{
    BindingLifecycle, CurrentRootSelection, DirectoryBinding, DirectoryId,
    LocalEntityIdentity, ReplicaBinding, ReplicaId, ReplicaObject,
    ReplicaPurpose, RootBinding, RootId, SnapshotRepositoryLocation, StorageLayout,
    StructurallyValidWorkingTreeCatalog, WorkingTreeCatalog,
};
use name_policy::LookupName;
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{DiskId, PoolId};

pub(crate) const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_ROOTS: usize = 256;
pub(crate) const MAX_SOURCE_CHAIN: usize = 32;
pub(crate) const MAX_CATALOG_ROWS: usize = 32_768;
pub(crate) const MAX_DIRECTORY_DEPTH: usize = 64;

#[derive(Debug, error_definition::Error)]
pub(crate) enum AdmissionError {
    #[error("working-tree material exceeds its admission budget")]
    Budget,
    #[error("decoding working-tree catalog")]
    Decode(#[source] storage_contract::Error),
    #[error("encoding working-tree catalog")]
    Encode(#[source] storage_contract::Error),
    #[error("working-tree records are not strictly ordered by their stable identity")]
    Order,
    #[error("working-tree storage layout does not match current authority")]
    Format,
    #[error("working-tree root does not match current disk registration")]
    Registration,
    #[error("working-tree registration identifier is invalid")]
    RegistrationIdentifier(#[source] storage_contract::Error),
    #[error("working-tree registration slot is invalid")]
    RegistrationSlot(#[source] name_policy::Error),
    #[error("working-tree source chain is incomplete, repeated or not current")]
    SourceChain,
    #[error("working-tree identity is reused")]
    DuplicateIdentity,
    #[error("working-tree entity is aliased or belongs to another filesystem")]
    PhysicalIdentity,
    #[error("working-tree parent is missing, cyclic or belongs to another root")]
    Parent,
    #[error("working-tree siblings have conflicting canonical names")]
    NameConflict,
    #[error("working-tree lifecycle disagrees with its parent or replica purpose")]
    Lifecycle,
    #[error("working-tree replica does not close its object and source bindings")]
    Replica,
    #[error("working-tree copies disagree about their recorded FILE commitment")]
    FileCommitment,
    #[error("allocating a working-tree directory identity")]
    Allocate(#[source] storage_contract::Error),
    #[error("directory target generation no longer matches the admitted root")]
    Generation,
    #[error("directory observation does not match the requested relative path")]
    Target,
}

/// The caller supplies this from independent current registry/topology state,
/// never by copying the candidate root being selected. Matching the tuple alone
/// proves neither catalog authenticity/completeness nor current physical FDs.
pub(crate) struct RootSelectionContext<'a> {
    pub(crate) storage_layout: StorageLayout,
    pub(crate) registration: &'a DiskRegistrationIdentity,
    pub(crate) source_chain: &'a [DiskRegistrationIdentity],
}

pub(crate) fn decode_catalog(bytes: &[u8]) -> Result<WorkingTreeCatalog, AdmissionError> {
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(AdmissionError::Budget);
    }
    storage_contract::decode_strict(bytes).map_err(AdmissionError::Decode)
}

/// Select only one structurally valid root against independent current context.
/// The constructor stays here so no caller can fabricate a selection from raw rows.
pub(crate) fn select_root<'a>(
    view: &'a StructurallyValidWorkingTreeCatalog<'a>,
    root_id: RootId,
    current: &RootSelectionContext<'_>,
) -> Result<CurrentRootSelection<'a>, AdmissionError> {
    let binding = view.roots.get(&root_id).copied().ok_or(AdmissionError::Parent)?;
    if binding.layout != current.storage_layout {
        return Err(AdmissionError::Format);
    }
    if binding.registration != *current.registration {
        return Err(AdmissionError::Registration);
    }
    if binding.source_chain != current.source_chain {
        return Err(AdmissionError::SourceChain);
    }
    Ok(CurrentRootSelection { tree: view, binding })
}

fn root_structure(binding: &RootBinding) -> Result<(), AdmissionError> {
    source_chain_structure(&binding.registration, &binding.source_chain)?;
    let mut ids = BTreeSet::new();
    let mut entities = BTreeSet::from([binding.root_entity]);
    for local in [
        &binding.roots.common_data,
        &binding.roots.deleted_data,
        &binding.roots.zxs_data,
    ] {
        if !ids.insert(local.directory_id) {
            return Err(AdmissionError::DuplicateIdentity);
        }
        if local.entity.device() != binding.root_entity.device()
            || !entities.insert(local.entity)
        {
            return Err(AdmissionError::PhysicalIdentity);
        }
    }
    Ok(())
}

fn source_chain_structure(
    owner: &DiskRegistrationIdentity,
    sources: &[DiskRegistrationIdentity],
) -> Result<(), AdmissionError> {
    registration(owner)?;
    if sources.len() > MAX_SOURCE_CHAIN {
        return Err(AdmissionError::Budget);
    }
    let mut chain = BTreeSet::new();
    let mut slots = BTreeSet::new();
    chain.insert(owner.disk_id.as_str());
    slots.insert(owner.slot_id.as_str());
    for source in sources {
        registration(source)?;
        if source.pool_id != owner.pool_id
            || !chain.insert(source.disk_id.as_str())
            || !slots.insert(source.slot_id.as_str())
        {
            return Err(AdmissionError::SourceChain);
        }
    }
    Ok(())
}

fn registration(identity: &DiskRegistrationIdentity) -> Result<(), AdmissionError> {
    identity.pool_id.parse::<PoolId>().map_err(AdmissionError::RegistrationIdentifier)?;
    identity.disk_id.parse::<DiskId>().map_err(AdmissionError::RegistrationIdentifier)?;
    name_policy::SlotId::parse(&identity.slot_id).map_err(AdmissionError::RegistrationSlot)?;
    Ok(())
}

/// Validate relationships within the supplied records without authenticating
/// their origin, matching current registrations or proving expected FILE coverage.
pub(crate) fn validate_catalog_structure(
    catalog: &WorkingTreeCatalog,
) -> Result<StructurallyValidWorkingTreeCatalog<'_>, AdmissionError> {
    counts(catalog)?;
    let roots = root_index(catalog)?;
    let directories = ordered_index(&catalog.directories, |row| row.directory_id)?;
    let replicas = ordered_index(&catalog.replicas, |row| row.replica_id)?;
    ordered_index(&catalog.containers, |row| row.replica_id)?;
    ordered_index(&catalog.snapshot_repositories, |row| row.replica_id)?;
    let view = StructurallyValidWorkingTreeCatalog {
        catalog, roots, directories, replicas,
    };
    let mut entities = BTreeSet::new();
    let mut directory_ids = BTreeSet::new();
    for root in &catalog.roots {
        for entity in [
            root.root_entity,
            root.roots.common_data.entity,
            root.roots.deleted_data.entity,
            root.roots.zxs_data.entity,
        ] {
            if !entities.insert(entity) {
                return Err(AdmissionError::PhysicalIdentity);
            }
        }
        for id in [
            root.roots.common_data.directory_id,
            root.roots.deleted_data.directory_id,
            root.roots.zxs_data.directory_id,
        ] {
            if !directory_ids.insert(id) || view.directories.contains_key(&id) {
                return Err(AdmissionError::DuplicateIdentity);
            }
        }
    }
    let mut names = BTreeSet::new();
    admit_directories(&view, &mut entities, &mut names)?;
    admit_replicas(&view, &mut entities)?;
    admit_objects(&view, &mut names)?;
    if storage_contract::canonical_json(catalog).map_err(AdmissionError::Encode)?.len()
        > MAX_CATALOG_BYTES
    {
        return Err(AdmissionError::Budget);
    }
    Ok(view)
}

fn counts(catalog: &WorkingTreeCatalog) -> Result<(), AdmissionError> {
    let lengths = [
        catalog.directories.len(),
        catalog.containers.len(),
        catalog.snapshot_repositories.len(),
        catalog.replicas.len(),
    ];
    if catalog.roots.len() > MAX_ROOTS
        || lengths.iter().any(|length| *length > MAX_CATALOG_ROWS)
        || lengths.iter().sum::<usize>() > MAX_CATALOG_ROWS
    {
        return Err(AdmissionError::Budget);
    }
    Ok(())
}

fn ordered_index<T, K: Copy + Ord>(
    rows: &[T],
    key: impl Fn(&T) -> K,
) -> Result<BTreeMap<K, &T>, AdmissionError> {
    let mut index = BTreeMap::new();
    let mut previous = None;
    for row in rows {
        let id = key(row);
        if previous.is_some_and(|before| before >= id) {
            return Err(AdmissionError::Order);
        }
        previous = Some(id);
        index.insert(id, row);
    }
    Ok(index)
}

fn root_index<'a>(
    catalog: &'a WorkingTreeCatalog,
) -> Result<BTreeMap<RootId, &'a RootBinding>, AdmissionError> {
    let roots = ordered_index(&catalog.roots, |row| row.root_id)?;
    let mut registrations = BTreeSet::new();
    let mut slots = BTreeSet::new();
    let pool = catalog.roots.first().map(|root| &root.registration.pool_id);
    for root in &catalog.roots {
        if root.layout != catalog.storage_layout {
            return Err(AdmissionError::Format);
        }
        root_structure(root)?;
        if Some(&root.registration.pool_id) != pool {
            return Err(AdmissionError::Registration);
        }
        if !registrations.insert(root.registration.disk_id.as_str())
            || !slots.insert(root.registration.slot_id.as_str())
        {
            return Err(AdmissionError::DuplicateIdentity);
        }
        for (offset, source) in root.source_chain.iter().enumerate() {
            let upstream = catalog.roots.iter().find(|candidate| {
                candidate.registration == *source
            }).ok_or(AdmissionError::SourceChain)?;
            if upstream.source_chain != root.source_chain[offset + 1..] {
                return Err(AdmissionError::SourceChain);
            }
        }
    }
    Ok(roots)
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct EntryKey(String);
type Names = BTreeSet<(DirectoryId, EntryKey)>;

fn management_name() -> EntryKey {
    EntryKey(name_policy::lookup_key(
        &layout::SNAPSHOT_REPOSITORY.as_os_str().to_string_lossy(),
    ))
}
fn insert_name(
    names: &mut Names,
    parent: DirectoryId,
    name: &LookupName,
) -> Result<(), AdmissionError> {
    let key = EntryKey(name.key());
    if key == management_name() || !names.insert((parent, key)) {
        return Err(AdmissionError::NameConflict);
    }
    Ok(())
}

/// Resolve the entire parent chain. The metadata-management anchor is never a
/// user-directory parent. Retained trees must retain one lifecycle operation.
fn parent_lifecycle(
    view: &StructurallyValidWorkingTreeCatalog<'_>,
    root_id: RootId,
    mut parent: DirectoryId,
) -> Result<BindingLifecycle, AdmissionError> {
    let root = view.roots.get(&root_id).ok_or(AdmissionError::Parent)?;
    let mut seen = BTreeSet::new();
    let mut lifecycle = None;
    loop {
        if parent == root.roots.common_data.directory_id {
            if lifecycle.is_some_and(|value| value != BindingLifecycle::Current) {
                return Err(AdmissionError::Lifecycle);
            }
            return Ok(BindingLifecycle::Current);
        }
        if parent == root.roots.deleted_data.directory_id {
            return match lifecycle {
                Some(value @ BindingLifecycle::Retained { .. }) => Ok(value),
                None => Err(AdmissionError::Lifecycle),
                Some(BindingLifecycle::Current) => Err(AdmissionError::Lifecycle),
            };
        }
        if seen.len() == MAX_DIRECTORY_DEPTH {
            return Err(AdmissionError::Budget);
        }
        if !seen.insert(parent) {
            return Err(AdmissionError::Parent);
        }
        let row = view.directories.get(&parent).ok_or(AdmissionError::Parent)?;
        if row.root_id != root_id {
            return Err(AdmissionError::Parent);
        }
        if lifecycle.is_some_and(|value| value != row.lifecycle) {
            return Err(AdmissionError::Lifecycle);
        }
        lifecycle = Some(row.lifecycle);
        parent = row.parent_directory_id;
    }
}

fn admit_directories(
    view: &StructurallyValidWorkingTreeCatalog<'_>,
    entities: &mut BTreeSet<LocalEntityIdentity>,
    names: &mut Names,
) -> Result<(), AdmissionError> {
    for row in &view.catalog.directories {
        let root = view.roots.get(&row.root_id).ok_or(AdmissionError::Parent)?;
        if row.entity.device() != root.root_entity.device() || !entities.insert(row.entity) {
            return Err(AdmissionError::PhysicalIdentity);
        }
        // Starting at self checks direct retained-root children without inventing
        // a current lifecycle for the deleted-data anchor.
        if parent_lifecycle(view, row.root_id, row.directory_id)? != row.lifecycle {
            return Err(AdmissionError::Lifecycle);
        }
        insert_name(names, row.parent_directory_id, &row.name)?;
    }
    Ok(())
}

fn admit_replicas(
    view: &StructurallyValidWorkingTreeCatalog<'_>,
    entities: &mut BTreeSet<LocalEntityIdentity>,
) -> Result<(), AdmissionError> {
    let mut current_objects = BTreeSet::new();
    let mut placements = BTreeMap::new();
    for replica in &view.catalog.replicas {
        if placements.insert((replica.root_id, replica.object), replica).is_some() {
            return Err(AdmissionError::Replica);
        }
    }
    for replica in &view.catalog.replicas {
        let root = view.roots.get(&replica.root_id).ok_or(AdmissionError::Replica)?;
        if replica.source_chain.len() > MAX_SOURCE_CHAIN {
            return Err(AdmissionError::Budget);
        }
        if replica.registration != root.registration || replica.source_chain != root.source_chain {
            return Err(AdmissionError::Replica);
        }
        if replica.entity.device() != root.root_entity.device()
            || !entities.insert(replica.entity)
        {
            return Err(AdmissionError::PhysicalIdentity);
        }
        match replica.purpose {
            ReplicaPurpose::Current => {
                if !replica.source_chain.is_empty() || !current_objects.insert(replica.object) {
                    return Err(AdmissionError::Replica);
                }
            }
            ReplicaPurpose::Backup => {
                let source = replica.source_chain.first().ok_or(AdmissionError::SourceChain)?;
                let source_root = view.catalog.roots.iter().find(|root| {
                    root.registration == *source
                }).ok_or(AdmissionError::SourceChain)?;
                let source_replica = placements.get(&(source_root.root_id, replica.object))
                    .ok_or(AdmissionError::Replica)?;
                if !matches!(
                    source_replica.purpose,
                    ReplicaPurpose::Current | ReplicaPurpose::Backup,
                ) {
                    return Err(AdmissionError::Replica);
                }
            }
            ReplicaPurpose::Retained => {}
        }
    }
    Ok(())
}

fn object_replica<'a>(
    view: &'a StructurallyValidWorkingTreeCatalog<'_>,
    root: RootId,
    parent: DirectoryId,
    lifecycle: BindingLifecycle,
    id: ReplicaId,
    object: ReplicaObject,
) -> Result<&'a ReplicaBinding, AdmissionError> {
    let root_binding = view.roots.get(&root).ok_or(AdmissionError::Parent)?;
    let parent_state = if parent == root_binding.roots.deleted_data.directory_id {
        match lifecycle {
            value @ BindingLifecycle::Retained { .. } => value,
            BindingLifecycle::Current => return Err(AdmissionError::Lifecycle),
        }
    } else {
        parent_lifecycle(view, root, parent)?
    };
    if parent_state != lifecycle {
        return Err(AdmissionError::Lifecycle);
    }
    let replica = view.replicas.get(&id).ok_or(AdmissionError::Replica)?;
    if replica.root_id != root || replica.object != object {
        return Err(AdmissionError::Replica);
    }
    match (lifecycle, replica.purpose) {
        (BindingLifecycle::Current, ReplicaPurpose::Current | ReplicaPurpose::Backup)
        | (BindingLifecycle::Retained { .. }, ReplicaPurpose::Retained) => Ok(replica),
        (BindingLifecycle::Current, ReplicaPurpose::Retained)
        | (BindingLifecycle::Retained { .. }, ReplicaPurpose::Current | ReplicaPurpose::Backup) => {
            Err(AdmissionError::Lifecycle)
        }
    }
}

fn admit_objects(
    view: &StructurallyValidWorkingTreeCatalog<'_>,
    names: &mut Names,
) -> Result<(), AdmissionError> {
    let mut used_replicas = BTreeSet::new();
    let mut repositories = BTreeMap::new();
    let mut files = BTreeMap::new();
    for row in &view.catalog.containers {
        let replica = object_replica(
            view, row.root_id, row.parent_directory_id, row.lifecycle,
            row.replica_id, ReplicaObject::Container(row.file_id),
        )?;
        if replica.operation_id != row.operation_id || !used_replicas.insert(row.replica_id) {
            return Err(AdmissionError::Replica);
        }
        if files.insert(row.file_id, row.file_authority)
            .is_some_and(|previous| previous != row.file_authority)
        {
            return Err(AdmissionError::FileCommitment);
        }
        insert_name(names, row.parent_directory_id, &row.name)?;
    }
    for row in &view.catalog.snapshot_repositories {
        let (parent, lifecycle) = match &row.location {
            SnapshotRepositoryLocation::Attached => {
                if parent_lifecycle(view, row.root_id, row.host_directory_id)?
                    != BindingLifecycle::Current
                {
                    return Err(AdmissionError::Lifecycle);
                }
                if !names.insert((row.host_directory_id, management_name())) {
                    return Err(AdmissionError::NameConflict);
                }
                (row.host_directory_id, BindingLifecycle::Current)
            }
            SnapshotRepositoryLocation::Retained {
                parent_directory_id,
                name,
                operation_id,
            } => {
                if *operation_id != row.operation_id {
                    return Err(AdmissionError::Lifecycle);
                }
                historical_host_root(view, row.root_id, row.host_directory_id)?;
                insert_name(names, *parent_directory_id, name)?;
                (*parent_directory_id, BindingLifecycle::Retained {
                    operation_id: *operation_id,
                })
            }
        };
        let replica = object_replica(
            view, row.root_id, parent, lifecycle,
            row.replica_id, ReplicaObject::SnapshotRepository(row.repository_id),
        )?;
        if replica.operation_id != row.operation_id || !used_replicas.insert(row.replica_id) {
            return Err(AdmissionError::Replica);
        }
        if repositories.insert(row.repository_id, row.material)
            .is_some_and(|previous| previous != row.material)
        {
            return Err(AdmissionError::Replica);
        }
    }
    if used_replicas.len() != view.replicas.len() {
        return Err(AdmissionError::Replica);
    }
    Ok(())
}

/// A retained repository's original host is provenance in the authenticated
/// operation, not its current placement. Deleting that ordinary host later does
/// not orphan the retained repository; a still-present ID cannot change roots.
fn historical_host_root(
    view: &StructurallyValidWorkingTreeCatalog<'_>,
    root_id: RootId,
    host: DirectoryId,
) -> Result<(), AdmissionError> {
    if let Some(directory) = view.directories.get(&host) {
        if directory.root_id != root_id {
            return Err(AdmissionError::Parent);
        }
    }
    for root in &view.catalog.roots {
        if host == root.roots.deleted_data.directory_id
            || host == root.roots.zxs_data.directory_id
            || (host == root.roots.common_data.directory_id && root.root_id != root_id)
        {
            return Err(AdmissionError::Parent);
        }
    }
    Ok(())
}

/// Evidence from an actual held path walk. This record does not replace the
/// walk's held descriptors or its before-decision and before-install checks.
pub(crate) struct ObservedDirectory {
    pub(crate) name: EntryName,
    pub(crate) entity: FileIdentity,
}

pub(crate) struct PreparedDirectoryChain {
    pub(crate) host_directory_id: DirectoryId,
    pub(crate) additions: Vec<DirectoryBinding>,
}

/// First preparation only. The operation owner persists these exact allocated
/// IDs before deciding or installing and reuses them during every recovery;
/// calling this again is not same-operation replay.
pub(crate) fn register_directory_chain(
    root: CurrentRootSelection<'_>,
    request: &storage_contract::working_tree::DirectoryTargetRequest,
    observations: &[ObservedDirectory],
) -> Result<PreparedDirectoryChain, AdmissionError> {
    use storage_contract::working_tree::DirectoryTargetPath;
    if request.root_id != root.root_id() {
        return Err(AdmissionError::Target);
    }
    if request.expected_root_generation != root.binding().generation {
        return Err(AdmissionError::Generation);
    }
    let components = match &request.path {
        DirectoryTargetPath::Root => &[][..],
        DirectoryTargetPath::Relative(path) => path.components(),
    };
    if components.len() != observations.len() || observations.len() > MAX_DIRECTORY_DEPTH {
        return Err(AdmissionError::Target);
    }
    let mut parent = root.common_data().directory_id;
    let mut additions = Vec::new();
    let mut observed_entities = BTreeSet::from([root.common_data().entity]);
    for (name, observation) in components.iter().zip(observations) {
        if observation.name.as_os_str().to_str() != Some(name.as_str()) {
            return Err(AdmissionError::Target);
        }
        let entity = LocalEntityIdentity::new(observation.entity.device, observation.entity.inode);
        if entity.device() != root.common_data().entity.device()
            || !observed_entities.insert(entity)
        {
            return Err(AdmissionError::PhysicalIdentity);
        }
        match root.entry(parent, &observation.name) {
            Some(crate::working_tree::RegisteredEntry::Directory(binding)) => {
                if binding.entity != entity || binding.name != *name {
                    return Err(AdmissionError::PhysicalIdentity);
                }
                parent = binding.directory_id;
            }
            Some(crate::working_tree::RegisteredEntry::Container(_))
            | Some(crate::working_tree::RegisteredEntry::SnapshotRepository(_)) => {
                return Err(AdmissionError::Target);
            }
            None => {
                if EntryKey(name.key()) == management_name() {
                    return Err(AdmissionError::NameConflict);
                }
                let directory_id = DirectoryId::generate().map_err(AdmissionError::Allocate)?;
                additions.push(DirectoryBinding {
                    directory_id,
                    root_id: root.root_id(),
                    parent_directory_id: parent,
                    name: name.clone(),
                    entity,
                    lifecycle: BindingLifecycle::Current,
                    operation_id: request.operation_id,
                });
                parent = directory_id;
            }
        }
    }
    Ok(PreparedDirectoryChain { host_directory_id: parent, additions })
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/working_tree_tests.rs"]
mod tests;
