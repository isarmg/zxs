//! Explicit fresh SYSTEM/bootstrap. Never invokes the legacy initializer.
//! A complete record admits exact root identities; unclaimed partial writes are
//! preserved and fenced, not repaired or adopted from their familiar names.
use crate::authority_selection::{FreshBootstrapError, SelectionGuard};
use crate::disk::DiskResources;
use crate::persist::layout;
use crate::{Engine, OperationId, ValidatedConfig};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory};
use platform_io::fs_safety::linux::{AdvisoryLock, MountPolicy, OpenAt2Root};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use storage_contract::working_tree::{
    DirectoryId, DiskRegistrationIdentity, LocalDirectoryBinding, LocalEntityIdentity,
    MaterialCommitment, RootBinding, RootId, StorageLayout, ThreeRootBinding,
};
use storage_contract::Generation;

const ROOTS: &str = "fresh-directory-roots-v1.json";
const COMPLETE: &str = "fresh-directory-bootstrap-complete-v1.json";
const MAX_BYTES: usize = 1024 * 1024;
type Result<T> = std::result::Result<T, FreshBootstrapError>;
fn engine_error(error: crate::Error) -> FreshBootstrapError {
    FreshBootstrapError::Engine(Box::new(error))
}
fn failure(error: FreshBootstrapError) -> crate::Error {
    crate::Error::FreshBootstrap(Box::new(error))
}
fn name(value: &str) -> Result<EntryName> {
    EntryName::new(value).map_err(FreshBootstrapError::Filesystem)
}
fn opened(directory: PrivateDirectory) -> Result<OpenAt2Root> {
    OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)
        .map_err(FreshBootstrapError::Filesystem)
}
fn entity(root: &OpenAt2Root) -> LocalEntityIdentity {
    let identity = root.identity();
    LocalEntityIdentity::new(identity.device, identity.inode)
}
fn identity(operation: OperationId, disk: &str, role: &str) -> [u8; 16] {
    let mut hash = blake3::Hasher::new_derive_key("zxs fresh directory logical identity v1");
    hash.update(operation.to_string().as_bytes());
    hash.update(&(disk.len() as u64).to_le_bytes());
    hash.update(disk.as_bytes());
    hash.update(role.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    bytes
}

pub(crate) struct FreshBootstrap {
    root: std::path::PathBuf,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    selection: SelectionGuard,
    _directory_locks: Vec<AdvisoryLock>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RootRecord {
    layout: StorageLayout,
    selection: MaterialCommitment,
    operation: OperationId,
    system_writer: LocalEntityIdentity,
    bootstrap_root: LocalEntityIdentity,
    bootstrap_writer: LocalEntityIdentity,
    disk_writers: Vec<LocalEntityIdentity>,
    roots: Vec<RootBinding>,
}
/// Original cold-admission evidence, never constructed from a later path lookup.
#[derive(Clone, Debug)]
pub(crate) struct CompletionProof {
    commitment: MaterialCommitment,
    system: LocalEntityIdentity,
    bootstrap_root: LocalEntityIdentity,
    bootstrap_writer: LocalEntityIdentity,
}

fn bootstrap_identity(system: &PrivateDirectory) -> Result<(LocalEntityIdentity, LocalEntityIdentity)> {
    let root = opened(system.try_clone().map_err(FreshBootstrapError::Filesystem)?)?;
    let child = root.open_directory(&name(
        crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY,
    )?.as_relative()).map_err(FreshBootstrapError::Filesystem)?;
    let child = OpenAt2Root::from_directory(child, MountPolicy::SameMount)
        .map_err(FreshBootstrapError::Filesystem)?;
    let child_identity = entity(&child);
    let directory = PrivateDirectory::from_directory(
        child.into_directory(), system.path().join(
            crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY,
        ),
    ).map_err(FreshBootstrapError::Filesystem)?;
    let writer = directory.open_payload(
        &layout::CONTROL_WRITER_LOCK, platform_io::fs_safety::PayloadOpenMode::Existing,
    ).map_err(FreshBootstrapError::Filesystem)?;
    let stat = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)
        .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
    if writer.metadata().map_err(|error| FreshBootstrapError::Filesystem(error.into()))?.len() != 0 {
        return Err(FreshBootstrapError::Changed);
    }
    Ok((child_identity, LocalEntityIdentity::new(stat.device, stat.inode)))
}

fn completion_commitment(bytes: &[u8]) -> MaterialCommitment {
    let mut hash = blake3::Hasher::new_derive_key("zxs directory bootstrap completion v1");
    hash.update(bytes);
    MaterialCommitment::from_digest(hash.finalize())
}

pub(crate) fn admit_bootstrap_handle(
    system: &PrivateDirectory, config: &ValidatedConfig,
    bootstrap: platform_io::fs_safety::linux::FileIdentity,
) -> crate::Result<LocalEntityIdentity> {
    let inspect = || -> Result<LocalEntityIdentity> {
        let proof = config.fresh_completion().ok_or(FreshBootstrapError::Incomplete)?;
        let root = opened(system.try_clone().map_err(FreshBootstrapError::Filesystem)?)?;
        let bytes = system.read_bounded(&name(COMPLETE)?, MAX_BYTES)
            .map_err(FreshBootstrapError::Filesystem)?;
        if entity(&root) != proof.system
            || completion_commitment(&bytes) != proof.commitment
            || LocalEntityIdentity::new(bootstrap.device, bootstrap.inode) != proof.bootstrap_root
        {
            return Err(FreshBootstrapError::Changed);
        }
        system.verify_identity().map_err(FreshBootstrapError::Filesystem)?;
        Ok(proof.bootstrap_writer)
    };
    inspect().map_err(failure)
}

impl RootRecord {
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = storage_contract::canonical_json(self).map_err(FreshBootstrapError::Codec)?;
        if bytes.len() > MAX_BYTES {
            return Err(FreshBootstrapError::Format);
        }
        Ok(bytes)
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(FreshBootstrapError::Format);
        }
        let record: Self = storage_contract::decode_strict(bytes)
            .map_err(FreshBootstrapError::Codec)?;
        if record.bytes()? != bytes {
            return Err(FreshBootstrapError::Format);
        }
        Ok(record)
    }
}

impl FreshBootstrap {
    /// All roots must already be explicitly selected private directories. This
    /// creates neither directories nor sidecars until SYSTEM selection is durable.
    pub(crate) fn prepare(
        root: &Path, config: ValidatedConfig, operation: OperationId,
    ) -> crate::Result<Self> {
        Self::prepare_inner(root, config, operation).map_err(failure)
    }
    fn prepare_inner(root: &Path, config: ValidatedConfig, operation: OperationId) -> Result<Self> {
        let system = PrivateDirectory::open_existing(root)
            .map_err(FreshBootstrapError::Filesystem)?;
        let directories = config.disks.iter().map(|disk| {
            PrivateDirectory::open_existing(&disk.root).map_err(FreshBootstrapError::Filesystem)
        }).collect::<Result<Vec<_>>>()?;
        Self::prepare_held(system, config, operation, directories)
    }

    // This is held bootstrap admission, not a complete held Engine initializer.
    // SYSTEM index/locator consumers still require a separate anchored-I/O change.
    fn prepare_held(
        system: PrivateDirectory, config: ValidatedConfig, operation: OperationId,
        directories: Vec<PrivateDirectory>,
    ) -> Result<Self> {
        if config.generation().is_some() || config.disks.len() > 256 {
            return Err(FreshBootstrapError::Changed);
        }
        crate::authority_selection::FreshSelection::require_primary(&config)?;
        let root = system.path().to_owned();
        let selection = SelectionGuard::acquire_held(system).map_err(engine_error)?;
        let mut directory_locks = Vec::new();
        for directory in &directories {
            let held = opened(directory.try_clone().map_err(FreshBootstrapError::Filesystem)?)?;
            directory_locks.push(AdvisoryLock::directory(held.directory())
                .map_err(FreshBootstrapError::Filesystem)?);
        }
        let selection = selection.select_fresh_held(&config, operation, &directories)?;
        let config = config.with_fresh_bootstrap(
            selection.fresh_selection().ok_or(FreshBootstrapError::Changed)?,
        ).map_err(engine_error)?;
        let system = selection.selected_directory().map_err(engine_error)?;
        let complete = match system.read_bounded(&name(COMPLETE)?, MAX_BYTES) {
            Ok(_) => true,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(FreshBootstrapError::Filesystem(error)),
        };
        let handles = directories.into_iter().map(|directory| {
            Ok(opened(directory)?.into_directory())
        }).collect::<Result<Vec<_>>>()?;
        let resources = Arc::new(if complete {
            DiskResources::recover_held(&config, handles)
        } else {
            DiskResources::initialize_held(&config, handles)
        }.map_err(engine_error)?);
        let admitted = selection.fresh_selection().ok_or(FreshBootstrapError::Changed)?;
        for (expected, disk) in admitted.disks.iter().zip(&config.disks) {
            let handle = resources.get(disk).map_err(engine_error)?;
            if expected.entity != LocalEntityIdentity::new(handle.seal.device, handle.seal.inode) {
                return Err(FreshBootstrapError::Changed);
            }
        }
        if complete {
            require_complete_held(&system, &config, &resources).map_err(engine_error)?;
        }
        Ok(Self {
            root, config, resources, selection,
            _directory_locks: directory_locks,
        })
    }

    /// Retry before the first payload-side write is exact. After any unrecorded
    /// partial initialization this returns Incomplete and leaves every byte intact.
    pub(crate) fn initialize(self) -> crate::Result<Engine> {
        self.initialize_inner().map_err(failure)
    }
    fn initialize_inner(self) -> Result<Engine> {
        let system = self.selection.selected_directory().map_err(engine_error)?;
        match system.read_bounded(&name(COMPLETE)?, MAX_BYTES) {
            Ok(_) => {
                require_complete(&self.root, &self.config).map_err(engine_error)?;
                let path = self.root.clone();
                drop(self);
                return Engine::open(&path).map_err(engine_error);
            }
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(FreshBootstrapError::Filesystem(error)),
        }
        let selector = self.selection.fresh_selection().ok_or(FreshBootstrapError::Changed)?;
        let operation = selector.operation;
        let selection = selector.commitment().map_err(engine_error)?;
        let selection_bytes = selector.canonical_bytes().map_err(engine_error)?;
        let system_reader = opened(system.try_clone().map_err(FreshBootstrapError::Filesystem)?)?;
        for entry in system_reader.entries().map_err(FreshBootstrapError::Filesystem)? {
            if entry.map_err(FreshBootstrapError::Filesystem)?.as_os_str()
                != crate::authority_selection::FRESH_SELECTOR
            {
                return Err(FreshBootstrapError::Incomplete);
            }
        }
        for disk in self.resources.handles() {
            if disk.root.entries(disk.root.path()).map_err(engine_error)?
                .next().transpose().map_err(FreshBootstrapError::Filesystem)?.is_some()
            {
                return Err(FreshBootstrapError::Incomplete);
            }
        }
        let locks = crate::engine::config::acquire_locks_selected(
            &self.root, &self.resources, self.selection,
        ).map_err(engine_error)?;
        let system_writer = locks.system_identity().map_err(engine_error)?;
        let system_writer = LocalEntityIdentity::new(system_writer.device, system_writer.inode);
        let mut disk_writers = Vec::new();
        let mut roots = Vec::new();
        for disk in &self.config.disks {
            let handle = self.resources.get(disk).map_err(engine_error)?;
            let root = &handle.root;
            locks.verify_disk(disk, root).map_err(engine_error)?;
            let writer = root.open_regular_entry(&layout::DISK_WRITER_LOCK).map_err(engine_error)?;
            let writer = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)
                .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
            disk_writers.push(LocalEntityIdentity::new(writer.device, writer.inode));
            root.write_json(&disk.root.join(layout::DISK_IDENTITY.as_path()), &handle.seal, false)
                .map_err(engine_error)?;
            root.write_atomic(
                &disk.root.join(crate::authority_selection::FRESH_DISK_SELECTOR),
                &selection_bytes, false,
            ).map_err(engine_error)?;
            let directory = opened(PrivateDirectory::from_directory(
                root.directory(&disk.root).map_err(engine_error)?, disk.root.clone(),
            ).map_err(FreshBootstrapError::Filesystem)?)?;
            let child = |entry: &EntryName, role: &str| -> Result<LocalDirectoryBinding> {
                let created = root.create_private_directory_synced(
                    &disk.root.join(entry.as_path()),
                ).map_err(engine_error)?;
                let held = root.directory(&disk.root.join(entry.as_path())).map_err(engine_error)?;
                let observed = platform_io::fs_safety::linux::FileIdentity::from_fd(&held)
                    .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
                if observed != created {
                    return Err(FreshBootstrapError::Changed);
                }
                Ok(LocalDirectoryBinding {
                    directory_id: DirectoryId::from_random(
                        identity(operation, &disk.disk_id, role),
                    ),
                    entity: LocalEntityIdentity::new(observed.device, observed.inode),
                })
            };
            let common_data = child(&layout::COMMON_DATA_ROOT, "common")?;
            let deleted_data = child(&layout::DELETED_DATA_ROOT, "deleted")?;
            let zxs_data = child(&layout::ZXS_DATA_ROOT, "zxs")?;
            roots.push(RootBinding {
                layout: StorageLayout::UnifiedWorkingTreeV1,
                root_id: RootId::from_random(identity(operation, &disk.disk_id, "root")),
                registration: DiskRegistrationIdentity {
                    pool_id: self.config.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                },
                source_chain: Vec::new(),
                root_entity: entity(&directory),
                roots: ThreeRootBinding { common_data, deleted_data, zxs_data },
                operation_id: operation,
                generation: Generation::new(1),
            });
            for path in [
                "histories", "metadata/application-log", "metadata/objects",
                "staging", "quarantine",
            ] {
                root.private_dir(&disk.root.join(path)).map_err(engine_error)?;
            }
            root.write_atomic(
                &disk.root.join("README.txt"),
                b"ZXS fresh directory snapshot v1. Legacy writers and migration forbidden.\n",
                false,
            ).map_err(engine_error)?;
            root.write_json(
                &disk.root.join("metadata/pool-membership/config.json"), &self.config, false,
            ).map_err(engine_error)?;
            root.sync_dir(&disk.root).map_err(engine_error)?;
        }
        crate::engine::disk_lifecycle::bootstrap::runtime::initialize(
            &self.root, &self.config, &self.resources,
        ).map_err(engine_error)?;
        let (bootstrap_root, bootstrap_writer) = bootstrap_identity(&system)?;
        let record = RootRecord {
            layout: StorageLayout::UnifiedWorkingTreeV1, selection, operation,
            system_writer, bootstrap_root, bootstrap_writer, disk_writers, roots,
        };
        let bytes = record.bytes()?;
        AtomicFile::create(&system, &name(ROOTS)?, &bytes)
            .map_err(FreshBootstrapError::Filesystem)?;

        let config = storage_contract::canonical_json(&self.config)
            .map_err(FreshBootstrapError::Codec)?;
        AtomicFile::create(&system, &name("config.json")?, &config)
            .map_err(FreshBootstrapError::Filesystem)?;
        system.create_child(&name("workspaces")?)
            .map_err(FreshBootstrapError::Filesystem)?;
        let mut journals = BTreeMap::new();
        for disk in self.config.disks.iter().filter(|disk| disk.metadata_member) {
            let journal = crate::journal::JournalWriter::create_at(
                &self.resources.get(disk).map_err(engine_error)?.root, disk,
            ).map_err(engine_error)?;
            journals.insert(disk.disk_id.clone(), journal);
        }
        let mut engine = Engine::assemble_initialized(
            self.root, self.config, self.resources, locks, journals,
        ).map_err(engine_error)?;
        // Completion is last. An index write interrupted before this record is
        // not permission to rebuild or adopt that index during fresh retry.
        AtomicFile::create(&system, &name(COMPLETE)?, &bytes)
            .map_err(FreshBootstrapError::Filesystem)?;
        verify_roots(&engine.config, &record, &selection_bytes, Some(&engine.disk_resources))?;
        let proof = require_complete_resources(&system, &engine.config, Some(&engine.disk_resources))
            .map_err(engine_error)?;
        engine.config = engine.config.clone().with_fresh_completion(proof);
        Ok(engine)
    }
}

pub(crate) struct AdmittedFreshRoot {
    binding: RootBinding,
}
impl AdmittedFreshRoot {
    pub(crate) fn binding(&self) -> &RootBinding {
        &self.binding
    }
}
fn verify_roots(
    config: &ValidatedConfig, record: &RootRecord, selector_bytes: &[u8],
    resources: Option<&DiskResources>,
) -> Result<()> {
    if config.fresh_bootstrap() != Some(record.selection)
        || record.roots.len() != config.disks.len()
        || record.disk_writers.len() != config.disks.len()
    {
        return Err(FreshBootstrapError::Changed);
    }
    for ((binding, writer), disk) in record.roots.iter().zip(&record.disk_writers)
        .zip(&config.disks)
    {
        if binding.registration.pool_id != config.pool_id
            || binding.registration.disk_id != disk.disk_id
            || binding.registration.slot_id != disk.slot_id
            || binding.registration.binding_generation != disk.binding_generation
            || binding.operation_id != record.operation
            || binding.generation != Generation::new(1)
            || !binding.source_chain.is_empty()
            || binding.root_id != RootId::from_random(
                identity(record.operation, &disk.disk_id, "root"),
            )
        {
            return Err(FreshBootstrapError::Changed);
        }
        let unopened;
        let root = match resources {
            Some(resources) => &resources.get(disk).map_err(engine_error)?.root,
            None => {
                // Cold validation runs before resource/writer creation. It
                // observes identities only and cannot grant a live I/O lease.
                unopened = crate::disk::DiskRoot::open(&disk.root).map_err(engine_error)?;
                &unopened
            }
        };
        let named_writer = root.open_regular_entry(&layout::DISK_WRITER_LOCK)
            .map_err(engine_error)?;
        let named_writer = platform_io::fs_safety::linux::FileIdentity::from_fd(&named_writer)
            .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
        if *writer != LocalEntityIdentity::new(named_writer.device, named_writer.inode) {
            return Err(FreshBootstrapError::Changed);
        }
        let directory = root.directory(&disk.root).map_err(engine_error)?;
        let held = platform_io::fs_safety::linux::FileIdentity::from_fd(&directory)
            .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
        let seal = crate::disk::decode_seal(&root.read_bytes(
            &disk.root.join(layout::DISK_IDENTITY.as_path()), MAX_BYTES,
        ).map_err(engine_error)?).map_err(engine_error)?;
        let expected = crate::disk::identity_at(config, disk, &directory).map_err(engine_error)?;
        crate::disk::validate_stored_seal(&seal, &expected).map_err(engine_error)?;
        if root.read_bytes(
            &disk.root.join(crate::authority_selection::FRESH_DISK_SELECTOR), MAX_BYTES,
        ).map_err(engine_error)? != selector_bytes
        {
            return Err(FreshBootstrapError::Changed);
        }
        if binding.root_entity != LocalEntityIdentity::new(held.device, held.inode) {
            return Err(FreshBootstrapError::Changed);
        }
        for (entry, role, child) in [
            (&*layout::COMMON_DATA_ROOT, "common", &binding.roots.common_data),
            (&*layout::DELETED_DATA_ROOT, "deleted", &binding.roots.deleted_data),
            (&*layout::ZXS_DATA_ROOT, "zxs", &binding.roots.zxs_data),
        ] {
            let directory = root.directory(&disk.root.join(entry.as_path())).map_err(engine_error)?;
            let observed = platform_io::fs_safety::linux::FileIdentity::from_fd(&directory)
                .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
            if child.entity != LocalEntityIdentity::new(observed.device, observed.inode)
                || child.directory_id != DirectoryId::from_random(
                    identity(record.operation, &disk.disk_id, role),
                )
            {
                return Err(FreshBootstrapError::Changed);
            }
        }
    }
    Ok(())
}
fn read_verified_roots(engine: &Engine) -> crate::Result<RootRecord> {
    let inspect = || -> Result<RootRecord> {
        let system = PrivateDirectory::open_existing(&engine.root)
            .map_err(FreshBootstrapError::Filesystem)?;
        let selector = crate::authority_selection::FreshSelection::read(&system)?
            .ok_or(FreshBootstrapError::Incomplete)?;
        selector.require_directory(&system)?;
        selector.require_config(&engine.config).map_err(engine_error)?;
        let bytes = system.read_bounded(&name(ROOTS)?, MAX_BYTES)
            .map_err(FreshBootstrapError::Filesystem)?;
        let complete = system.read_bounded(&name(COMPLETE)?, MAX_BYTES)
            .map_err(FreshBootstrapError::Filesystem)?;
        if bytes != complete || engine.config.fresh_completion()
            .is_none_or(|proof| proof.commitment != completion_commitment(&complete))
        {
            return Err(FreshBootstrapError::Changed);
        }
        let record = RootRecord::decode(&bytes)?;
        if record.operation != selector.operation
            || record.selection != selector.commitment().map_err(engine_error)?
        {
            return Err(FreshBootstrapError::Changed);
        }
        let writer = system.open_payload(
            &layout::CONTROL_WRITER_LOCK, platform_io::fs_safety::PayloadOpenMode::Existing,
        ).map_err(FreshBootstrapError::Filesystem)?;
        let named = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)
            .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
        if named != engine._locks.system_identity().map_err(engine_error)?
            || record.system_writer != LocalEntityIdentity::new(named.device, named.inode)
        {
            return Err(FreshBootstrapError::Changed);
        }
        verify_roots(
            &engine.config, &record, &selector.canonical_bytes().map_err(engine_error)?,
            Some(&engine.disk_resources),
        )?;
        system.verify_identity().map_err(FreshBootstrapError::Filesystem)?;
        Ok(record)
    };
    inspect().map_err(failure)
}

pub(crate) fn fresh_root(engine: &Engine, id: RootId) -> crate::Result<AdmittedFreshRoot> {
    let record = read_verified_roots(engine)?;
    let binding = record.roots.into_iter().find(|binding| binding.root_id == id)
        .ok_or_else(|| failure(FreshBootstrapError::Changed))?;
    Ok(AdmittedFreshRoot { binding })
}

impl Engine {
    /// Read current selectable descriptions from the original fresh SYSTEM and
    /// DISK evidence. These values are not write capabilities: callers select a
    /// RootId, and mutation must independently re-admit that root at use time.
    /// Missing or changed evidence is an error, never an omitted root.
    pub fn directory_storage_roots(&self) -> crate::Result<Vec<RootBinding>> {
        Ok(read_verified_roots(self)?.roots)
    }

    /// Initialize only explicitly selected, existing empty private roots in the
    /// new directory format. Old and interrupted unclaimed material is preserved.
    pub fn initialize_directory_storage(
        root: &Path, config: crate::Config, operation: OperationId,
    ) -> crate::Result<Self> {
        FreshBootstrap::prepare(root, ValidatedConfig::try_from(config)?, operation)?.initialize()
    }

    pub(crate) fn fresh_directory_root(&self, id: RootId) -> crate::Result<AdmittedFreshRoot> {
        fresh_root(self, id)
    }
}

/// Must run before writer-sidecar creation or writable index admission on cold open.
pub(crate) fn require_complete(
    root: &Path, config: &ValidatedConfig,
) -> crate::Result<CompletionProof> {
    let system = PrivateDirectory::open_existing(root)
        .map_err(|error| failure(FreshBootstrapError::Filesystem(error)))?;
    require_complete_resources(&system, config, None)
}

fn require_complete_held(
    system: &PrivateDirectory, config: &ValidatedConfig, resources: &DiskResources,
) -> crate::Result<()> {
    require_complete_resources(system, config, Some(resources)).map(|_| ())
}

fn require_complete_resources(
    system: &PrivateDirectory, config: &ValidatedConfig, resources: Option<&DiskResources>,
) -> crate::Result<CompletionProof> {
    let inspect = || -> Result<CompletionProof> {
        let bytes = system.read_bounded(&name(ROOTS)?, MAX_BYTES)
            .map_err(FreshBootstrapError::Filesystem)?;
        let complete = system.read_bounded(&name(COMPLETE)?, MAX_BYTES)
            .map_err(FreshBootstrapError::Filesystem)?;
        if bytes != complete {
            return Err(FreshBootstrapError::Changed);
        }
        let selector = crate::authority_selection::FreshSelection::read(&system)?
            .ok_or(FreshBootstrapError::Incomplete)?;
        selector.require_directory(&system)?;
        selector.require_config(config).map_err(engine_error)?;
        let record = RootRecord::decode(&bytes)?;
        if record.operation != selector.operation
            || record.selection != selector.commitment().map_err(engine_error)?
        {
            return Err(FreshBootstrapError::Changed);
        }
        let writer = system.open_payload(
            &layout::CONTROL_WRITER_LOCK, platform_io::fs_safety::PayloadOpenMode::Existing,
        ).map_err(FreshBootstrapError::Filesystem)?;
        let observed = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)
            .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
        if record.system_writer != LocalEntityIdentity::new(observed.device, observed.inode) {
            return Err(FreshBootstrapError::Changed);
        }
        verify_roots(
            config, &record, &selector.canonical_bytes().map_err(engine_error)?, resources,
        )?;
        let (bootstrap_root, bootstrap_writer) = bootstrap_identity(system)?;
        if bootstrap_root != record.bootstrap_root || bootstrap_writer != record.bootstrap_writer {
            return Err(FreshBootstrapError::Changed);
        }
        system.verify_identity().map_err(FreshBootstrapError::Filesystem)?;
        Ok(CompletionProof {
            commitment: completion_commitment(&complete), system: selector.system,
            bootstrap_root, bootstrap_writer,
        })
    };
    inspect().map_err(failure)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/fresh_directory_bootstrap_tests.rs"]
mod tests;
