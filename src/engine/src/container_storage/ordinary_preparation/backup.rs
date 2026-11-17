//! Native Backup's isolated target occurrences. This does not fabricate an
//! ordinary DiskHandle before the actual native target seal/writer exists.
//! Creation uses the same exclusive directory and five-area kernels as ordinary
//! publication. Only its exact original-member source can invoke preparation.
use super::*;
use crate::engine::fresh_layout::directory_error;
use crate::engine::membership_transition::{BackupLayoutPreparation, BackupLayoutSource};
use std::sync::atomic::AtomicBool;
use crate::history_package::preparation_checkpoint;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::EntryName;
use storage_contract::{EntryId, OperationId};
use storage_contract::file_container::{
    ContainerPrefix, ContainerReplica, NamespaceDirectoryParent as Parent,
    NamespaceDirectoryReplica as DirectoryReplica,
};
use storage_contract::working_tree::{BindingLifecycle, DirectoryId, RootId};

const MAX_ENTITIES: usize = 100_000;
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// Descriptive feedback only. Decoding it never grants a directory handle,
/// namespace installation, generation, payload proof or writer capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct BackupTargetLayout {
    disk: DiskId,
    root: RootId,
    operation: OperationId,
    prefixes: Vec<ContainerPrefix>,
    directories: BTreeMap<EntryId, DirectoryReplica>,
    files: BTreeMap<FileId, ContainerReplica>,
}

mod originals;
pub(crate) use originals::{BackupOriginalImage, BackupPayloadVersion};

mod serving;
pub(crate) use serving::{BackupServingImage, BackupServingVersion};

pub(crate) struct BackupLayoutPlan(BackupTargetLayout);

impl BackupLayoutPlan {
    pub(crate) fn bytes(&self) -> Result<usize> {
        Ok(self.0.bytes()?.len())
    }
}

struct HeldNode {
    root: DiskRoot,
    installed: bool,
}
struct Observed {
    _prefixes: Vec<(HeldNode, areas::HeldAreas)>,
    _directories: BTreeMap<EntryId, HeldNode>,
    _files: BTreeMap<FileId, HeldNode>,
    _reservation: crate::engine::PayloadProofReservation,
}

impl BackupTargetLayout {
    /// All targets retain this exact plan before the first mkdir. It contains
    /// only a fixed-width budget shape and has no serializable/public constructor.
    pub(crate) fn plan(
        preparation: &BackupLayoutPreparation<'_>, stop: &AtomicBool,
    ) -> Result<BackupLayoutPlan> {
        let source = preparation.source();
        source.require(stop)?;
        let result = Self::recipe(source, stop)?;
        let configured = crate::disk_names::configuration::ConfiguredDiskNames::new(
            source.config(),
        ).map_err(crate::Error::ConfiguredDiskName)?;
        let path = configured.source_path(source.disk()).map_err(crate::Error::ConfiguredDiskName)?;
        let components = layout::container::source_components(&path).collect::<Vec<_>>();
        preflight(source, &result, &management(source)?, &components)?;
        source.require(stop)?;
        Ok(BackupLayoutPlan(result))
    }

    pub(crate) fn prepare(
        preparation: &BackupLayoutPreparation<'_>, plan: BackupLayoutPlan, stop: &AtomicBool,
    ) -> Result<Self> {
        let source = preparation.source();
        source.require(stop)?;
        let mut result = plan.0;
        if result.disk != source.disk() || result.root != source.binding()?.root_id
            || result.operation != source.operation()
            || result.files.keys().ne(source.state().file_containers.keys())
            || result.directories.len() != source.state().namespace_directories.len()
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let before = result.bytes()?.len();
        let reservation = reserve(result.descriptors()?)?;
        let root = source.root()?;
        let management = management(source)?;
        // Without an original-member feedback record, previous preparations
        // remain conflicts. No directory is adopted by its name or its bytes.
        for entry in management.entries(management.path())? {
            let entry = entry.map_err(crate::disk::fs_error)?;
            if entry != *layout::COMMON_DATA_ROOT && entry != *layout::DELETED_DATA_ROOT {
                return Err(Failure::BackupRootUnrecorded.into());
            }
        }
        let configured = crate::disk_names::configuration::ConfiguredDiskNames::new(
            source.config(),
        ).map_err(crate::Error::ConfiguredDiskName)?;
        let path = configured.source_path(source.disk()).map_err(crate::Error::ConfiguredDiskName)?;
        let components = layout::container::source_components(&path).collect::<Vec<_>>();
        preflight(source, &result, &management, &components)?;
        let mut held_prefixes: Vec<(HeldNode, areas::HeldAreas)> = Vec::new();
        for (record, component) in result.prefixes.iter_mut().zip(&components) {
            preparation_checkpoint(stop)?;
            source.root()?;
            let parent = held_prefixes.last().map(|(node, _)| &node.root).unwrap_or(root);
            absent(parent, component)?;
            let (directory, identity) = prefix::create_exclusive(&management,
                &layout::container::prefix_preparation(record.preparation))?;
            let areas = areas::HeldAreas::create(&directory)?;
            record.entity = entity(identity);
            record.areas = areas.record();
            held_prefixes.push((HeldNode { root: directory, installed: false }, areas));
        }
        let common = held_prefixes.last().ok_or(Failure::BackupLayoutEvidence)?.1.common();
        let mut directories = BTreeMap::new();
        for entry in ordered_directories(source.state())? {
            preparation_checkpoint(stop)?;
            source.root()?;
            let logical = &source.state().namespace_directories[&entry.to_string()];
            let parent = parent(common, &directories, logical.parent)?;
            let record = result.directories.get_mut(&entry).ok_or(Failure::BackupLayoutEvidence)?;
            absent(parent, &layout::namespace::directory(&logical.name))?;
            let (directory, identity) = prefix::create_exclusive(&management,
                &layout::container::namespace_preparation(record.preparation))?;
            record.source_prefixes = result.prefixes.clone();
            record.parent_entity = entity(parent.directory_identity(parent.path())?);
            record.entity = entity(identity);
            directories.insert(entry, HeldNode { root: directory, installed: false });
        }
        let mut files = BTreeMap::new();
        for (file, record) in &mut result.files {
            preparation_checkpoint(stop)?;
            source.root()?;
            let logical = &source.state().file_containers[file];
            let parent = parent(common, &directories,
                logical.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?)?;
            absent(parent, &layout::namespace::directory(&logical.namespace.name))?;
            let (directory, identity) = prefix::create_exclusive(&management,
                &layout::container::preparation(record.preparation))?;
            directory.private_dir(&directory.path().join(layout::VERSIONS.as_path()))?;
            directory.sync_dir(directory.path())?;
            record.prefixes = result.prefixes.clone();
            record.entity = entity(identity);
            files.insert(*file, HeldNode { root: directory, installed: false });
        }
        // Fixed-width entities make this equality exact, not an estimate that
        // may grow after physical work. The zero shape never leaves this call.
        if result.bytes()?.len() != before {
            return Err(Failure::ContainerBudget.into());
        }
        source.require(stop)?;
        drop((held_prefixes, directories, files, reservation));
        result.observe(source, stop)?;
        Ok(result)
    }

    /// Read-only stage/final observation. A valid source and every expected
    /// inode are required; neither missing side authorizes recreation.
    pub(crate) fn observe(&self, source: &BackupLayoutSource<'_>, stop: &AtomicBool) -> Result<()> {
        self.hold(source, stop).map(|_| ())
    }

    fn hold(&self, source: &BackupLayoutSource<'_>, stop: &AtomicBool) -> Result<Observed> {
        source.require(stop)?;
        self.require_shape(source)?;
        let reservation = reserve(self.descriptors()?)?;
        let root = source.root()?;
        let management = management(source)?;
        let configured = crate::disk_names::configuration::ConfiguredDiskNames::new(
            source.config(),
        ).map_err(crate::Error::ConfiguredDiskName)?;
        let path = configured.source_path(source.disk()).map_err(crate::Error::ConfiguredDiskName)?;
        let mut prefixes: Vec<(HeldNode, areas::HeldAreas)> = Vec::new();
        for (record, component) in self.prefixes.iter()
            .zip(layout::container::source_components(&path))
        {
            preparation_checkpoint(stop)?;
            source.root()?;
            let (parent, installed) = prefixes.last()
                .map(|(node, _)| (&node.root, node.installed)).unwrap_or((root, true));
            let node = locate(
                &management, &layout::container::prefix_preparation(record.preparation),
                parent, &component, installed, record.entity)?;
            let areas = areas::HeldAreas::observe(&node.root, record.areas)?;
            prefixes.push((node, areas));
        }
        let last = prefixes.last().ok_or(Failure::BackupLayoutEvidence)?;
        let common = last.1.common();
        let common_installed = last.0.installed;
        let mut directories = BTreeMap::new();
        for entry in ordered_directories(source.state())? {
            preparation_checkpoint(stop)?;
            let logical = &source.state().namespace_directories[&entry.to_string()];
            let record = &self.directories[&entry];
            let parent = parent(common, &directories, logical.parent)?;
            if entity(parent.directory_identity(parent.path())?) != record.parent_entity {
                return Err(Failure::ContainerIdentity.into());
            }
            let installed = parent_installed(common_installed, &directories, logical.parent)?;
            let node = locate(&management,
                &layout::container::namespace_preparation(record.preparation), parent,
                &layout::namespace::directory(&logical.name), installed, record.entity)?;
            directories.insert(entry, node);
        }
        let mut files = BTreeMap::new();
        for (file, record) in &self.files {
            preparation_checkpoint(stop)?;
            let logical = &source.state().file_containers[file];
            let parent = parent(common, &directories,
                logical.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?)?;
            let installed = parent_installed(common_installed, &directories,
                logical.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?)?;
            let node = locate(&management, &layout::container::preparation(record.preparation),
                parent, &layout::namespace::directory(&logical.namespace.name),
                installed, record.entity)?;
            node.root.private_child(&node.root.path().join(layout::VERSIONS.as_path()))?;
            files.insert(*file, node);
        }
        let held = Observed { _prefixes: prefixes, _directories: directories,
            _files: files, _reservation: reservation };
        source.require(stop)?;
        Ok(held)
    }

    fn recipe(source: &BackupLayoutSource<'_>, stop: &AtomicBool) -> Result<Self> {
        let state = source.state();
        crate::engine::fresh_layout::containers::validate_state(state)?;
        crate::engine::fresh_layout::directories::validate_state(state)?;
        if !state.file_deletions.is_empty() || !state.directory_git_repositories.is_empty() {
            return Err(Failure::BackupLayoutUnsupported.into());
        }
        let binding = source.binding()?;
        let owner = binding.source_chain.last().ok_or(Failure::BackupLayoutEvidence)?
            .disk_id.parse::<DiskId>()?;
        let prefixes = binding.source_chain.iter().map(|registration| {
            Ok(ContainerPrefix {
                source_disk: registration.disk_id.parse()?, preparation: DirectoryId::generate()?,
                entity: ContainerEntity::new(0, 0),
                areas: storage_contract::file_container::ContainerAreas::budget(),
            })
        }).collect::<Result<Vec<_>>>()?;
        let mut result = Self { disk: source.disk(), root: binding.root_id,
            operation: source.operation(), prefixes, directories: BTreeMap::new(),
            files: BTreeMap::new() };
        for logical in state.namespace_directories.values() {
            preparation_checkpoint(stop)?;
            if logical.owner_disk != owner || logical.replicas.contains_key(&source.disk()) {
                return Err(Failure::BackupLayoutUnsupported.into());
            }
            result.directories.insert(logical.entry, DirectoryReplica {
                root_id: binding.root_id, source_chain: binding.source_chain.clone(),
                source_prefixes: result.prefixes.clone(), parent_entity: ContainerEntity::new(0, 0),
                preparation: DirectoryId::generate()?, entity: ContainerEntity::new(0, 0),
                lifecycle: BindingLifecycle::Current, created_by: source.operation(),
            });
        }
        for (file, logical) in state.file_containers.iter() {
            preparation_checkpoint(stop)?;
            if logical.owner_disk != owner || logical.replicas.contains_key(&source.disk())
                || !matches!(logical.working,
                    storage_contract::file_draft::FileWorkingState::Automatic { .. })
            {
                return Err(Failure::BackupLayoutUnsupported.into());
            }
            result.files.insert(*file, ContainerReplica { root_id: binding.root_id,
                prefixes: result.prefixes.clone(), preparation: DirectoryId::generate()?,
                entity: ContainerEntity::new(0, 0) });
        }
        result.descriptors()?;
        result.bytes()?;
        Ok(result)
    }

    fn require_shape(&self, source: &BackupLayoutSource<'_>) -> Result<()> {
        self.require_records(source.binding()?, source.state(), source.operation())
    }

    pub(crate) fn require_binding(
        &self, binding: &storage_contract::working_tree::RootBinding, operation: OperationId,
    ) -> Result<()> {
        if self.disk != binding.registration.disk_id.parse::<DiskId>()?
            || self.root != binding.root_id || self.operation != operation
            || binding.operation_id != operation
            || self.prefixes.is_empty()
            || self.prefixes.iter().map(|p| p.source_disk.to_string())
                .ne(binding.source_chain.iter().map(|r| r.disk_id.clone()))
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        Ok(())
    }

    fn require_records(
        &self, binding: &storage_contract::working_tree::RootBinding,
        state: &State, operation: OperationId,
    ) -> Result<()> {
        self.require_binding(binding, operation)?;
        let owner = binding.source_chain.last().ok_or(Failure::BackupLayoutEvidence)?
            .disk_id.parse::<DiskId>()?;
        if self.files.keys().ne(state.file_containers.keys())
            || self.directories.len() != state.namespace_directories.len()
            || !state.file_deletions.is_empty() || !state.directory_git_repositories.is_empty()
            || state.file_containers.values().any(|record| record.owner_disk != owner
                || record.replicas.contains_key(&self.disk)
                || !matches!(record.working,
                    storage_contract::file_draft::FileWorkingState::Automatic { .. }))
            || state.namespace_directories.values().any(|record| record.owner_disk != owner
                || record.replicas.contains_key(&self.disk))
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let mut ids = BTreeSet::new();
        let mut entities = BTreeSet::new();
        let mut require = |id, values: &[ContainerEntity]| -> Result<()> {
            if !ids.insert(id) || values.iter().any(|value| {
                value.inode() == 0 || !entities.insert(*value)
            }) {
                return Err(Failure::BackupLayoutEvidence.into());
            }
            Ok(())
        };
        for prefix in &self.prefixes {
            let mut values = vec![prefix.entity];
            values.extend(prefix.areas.entities());
            require(prefix.preparation, &values)?;
        }
        for logical in state.namespace_directories.values() {
            let record = self.directories.get(&logical.entry).ok_or(Failure::BackupLayoutEvidence)?;
            if record.root_id != self.root || record.source_chain != binding.source_chain
                || record.source_prefixes != self.prefixes || record.created_by != self.operation
                || record.lifecycle != BindingLifecycle::Current
            {
                return Err(Failure::BackupLayoutEvidence.into());
            }
            require(record.preparation, &[record.entity])?;
        }
        for record in self.files.values() {
            if record.root_id != self.root || record.prefixes != self.prefixes {
                return Err(Failure::BackupLayoutEvidence.into());
            }
            require(record.preparation, &[record.entity])?;
        }
        self.bytes()?;
        Ok(())
    }

    pub(crate) fn require_published(
        &self, binding: &storage_contract::working_tree::RootBinding,
        original: &State, published: &State, operation: OperationId,
    ) -> Result<()> {
        self.require_records(binding, original, operation)?;
        for (file, replica) in &self.files {
            let next = published.file_containers.get(file).ok_or(Failure::BackupLayoutEvidence)?;
            let before = original.file_containers.get(file).ok_or(Failure::BackupLayoutEvidence)?;
            let mut retained = next.clone();
            retained.replicas.retain(|disk, _| before.replicas.contains_key(disk));
            if &retained != before || next.replicas.get(&self.disk) != Some(replica) {
                return Err(Failure::BackupLayoutEvidence.into());
            }
        }
        for (entry, replica) in &self.directories {
            let next = published.namespace_directories.get(&entry.to_string())
                .ok_or(Failure::BackupLayoutEvidence)?;
            let before = original.namespace_directories.get(&entry.to_string())
                .ok_or(Failure::BackupLayoutEvidence)?;
            let mut retained = next.clone();
            retained.replicas.retain(|disk, _| before.replicas.contains_key(disk));
            if &retained != before || next.replicas.get(&self.disk) != Some(replica) {
                return Err(Failure::BackupLayoutEvidence.into());
            }
        }
        Ok(())
    }

    /// Pure feedback attachment. The original decision caller first requires
    /// both complete tables to equal the verified source, then applies every
    /// selected target once. This method has no filesystem or writer effect.
    pub(crate) fn attach(
        &self, binding: &storage_contract::working_tree::RootBinding,
        source: &State, next: &mut State, operation: OperationId,
    ) -> Result<()> {
        self.require_records(binding, source, operation)?;
        if next.operations.get(&operation).is_none_or(|result| result.sequence != next.sequence) {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        for (entry, replica) in &self.directories {
            let previous = source.namespace_directories.get(&entry.to_string())
                .ok_or(Failure::BackupLayoutEvidence)?;
            let record = next.namespace_directories.get_mut(&entry.to_string())
                .ok_or(Failure::BackupLayoutEvidence)?;
            let mut retained = record.clone();
            retained.replicas.retain(|disk, _| previous.replicas.contains_key(disk));
            if &retained != previous
                || record.replicas.insert(self.disk, replica.clone()).is_some()
            {
                return Err(Failure::BackupLayoutEvidence.into());
            }
        }
        for (file, replica) in &self.files {
            let previous = source.file_containers.get(file).ok_or(Failure::BackupLayoutEvidence)?;
            let record = next.file_containers.get_mut(file).ok_or(Failure::BackupLayoutEvidence)?;
            let mut retained = record.clone();
            retained.replicas.retain(|disk, _| previous.replicas.contains_key(disk));
            if &retained != previous
                || record.replicas.insert(self.disk, replica.clone()).is_some()
            {
                return Err(Failure::BackupLayoutEvidence.into());
            }
        }
        Ok(())
    }

    fn descriptors(&self) -> Result<usize> {
        let count = self.prefixes.len().checked_mul(6)
            .and_then(|v| v.checked_add(self.directories.len()))
            .and_then(|v| v.checked_add(self.files.len()))
            .and_then(|v| v.checked_add(1)).ok_or(Failure::ContainerBudget)?;
        if count > MAX_ENTITIES {
            return Err(Failure::ContainerBudget.into());
        }
        Ok(count)
    }

    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = crate::canonical_json(self)?;
        if bytes.len() > MAX_BYTES {
            return Err(Failure::ContainerBudget.into());
        }
        Ok(bytes)
    }
}

fn reserve(count: usize) -> Result<crate::engine::PayloadProofReservation> {
    crate::engine::payload_proof_budget().reserve(count)
        .ok_or_else(|| Failure::ContainerBudget.into())
}
fn entity(value: FileIdentity) -> ContainerEntity {
    ContainerEntity::new(value.device, value.inode)
}
fn identity(value: ContainerEntity) -> FileIdentity {
    FileIdentity { device: value.device(), inode: value.inode() }
}
fn management(source: &BackupLayoutSource<'_>) -> Result<DiskRoot> {
    let expected = source.binding()?.roots.zxs_data.entity;
    Ok(source.root()?.private_entry_matching(&layout::ZXS_DATA_ROOT,
        FileIdentity { device: expected.device(), inode: expected.inode() })
        .map_err(directory_error)?)
}
fn absent(root: &DiskRoot, name: &EntryName) -> Result<()> {
    if root.optional_metadata(&root.path().join(name.as_path()))?.is_some() {
        return Err(Failure::ContainerIdentity.into());
    }
    Ok(())
}
fn parent<'a>(
    common: &'a DiskRoot, directories: &'a BTreeMap<EntryId, HeldNode>, selected: Parent,
) -> Result<&'a DiskRoot> {
    match selected {
        Parent::Root => Ok(common),
        Parent::Directory(entry) => directories.get(&entry).map(|node| &node.root)
            .ok_or_else(|| Failure::BackupLayoutEvidence.into()),
    }
}
fn parent_installed(
    common: bool, directories: &BTreeMap<EntryId, HeldNode>, selected: Parent,
) -> Result<bool> {
    match selected {
        Parent::Root => Ok(common),
        Parent::Directory(entry) => directories.get(&entry).map(|node| node.installed)
            .ok_or_else(|| Failure::BackupLayoutEvidence.into()),
    }
}
fn ordered_directories(state: &State) -> Result<Vec<EntryId>> {
    let mut result = Vec::with_capacity(state.namespace_directories.len());
    for record in state.namespace_directories.values() {
        let depth = crate::engine::fresh_layout::directories::chain(state, record.entry)?.len();
        result.push((depth, record.entry));
    }
    result.sort();
    Ok(result.into_iter().map(|(_, entry)| entry).collect())
}
fn locate(
    management: &DiskRoot, stage: &EntryName, parent: &DiskRoot, name: &EntryName,
    parent_installed: bool, expected: ContainerEntity,
) -> Result<HeldNode> {
    let staged = management.optional_metadata(&management.path().join(stage.as_path()))?.is_some();
    let named = parent.optional_metadata(&parent.path().join(name.as_path()))?.is_some();
    let (root, installed) = match (staged, named, parent_installed) {
        (true, false, _) => (management.private_entry_matching(stage, identity(expected))
            .map_err(directory_error)?, false),
        (false, true, true) => (parent.private_entry_matching(name, identity(expected))
            .map_err(directory_error)?, true),
        _ => return Err(Failure::ContainerIdentity.into()),
    };
    Ok(HeldNode { root, installed })
}
fn preflight(
    source: &BackupLayoutSource<'_>, record: &BackupTargetLayout,
    management: &DiskRoot, components: &[EntryName],
) -> Result<()> {
    let root = source.root()?;
    for component in components {
        super::super::version_names::check_target_name(root, component)?;
    }
    for prefix in &record.prefixes {
        super::super::version_names::check_target_name(management,
            &layout::container::prefix_preparation(prefix.preparation))?;
    }
    for (entry, value) in &record.directories {
        super::super::version_names::check_target_name(management,
            &layout::container::namespace_preparation(value.preparation))?;
        super::super::version_names::check_target_name(root,
            &layout::namespace::directory(&source.state().namespace_directories[
                &entry.to_string()].name))?;
    }
    for (file, value) in &record.files {
        super::super::version_names::check_target_name(management,
            &layout::container::preparation(value.preparation))?;
        super::super::version_names::check_target_name(root,
            &layout::namespace::directory(&source.state().file_containers[file].namespace.name))?;
    }
    // Each target can have a different component budget. Admit the actual
    // installed BODY name before any target namespace preparation is created.
    for version in source.state().versions.values() {
        let file = version.file_id.parse::<FileId>()?;
        let logical = source.state().file_containers.get(&file)
            .ok_or(Failure::BackupLayoutEvidence)?;
        let name = name_policy::Name::parse(logical.namespace.name.as_str())?;
        let leaf = super::super::version_names::installed_version_leaf(
            version.storage_id, &name,
        )?;
        super::super::version_names::check_target_name(root, &leaf)?;
    }
    let required = (MAX_BYTES as u64).checked_add(record.descriptors()? as u64 * 8192)
        .and_then(|v| v.checked_add(16 * 1024 * 1024)).ok_or(Failure::ContainerBudget)?;
    if root.available()? < required {
        return Err(Failure::BackupRootSpace.into());
    }
    Ok(())
}
