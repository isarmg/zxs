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
    disk_writers: Vec<LocalEntityIdentity>,
    roots: Vec<RootBinding>,
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
        if config.generation().is_some() || config.disks.len() > 256 {
            return Err(FreshBootstrapError::Changed);
        }
        let selection = SelectionGuard::acquire(root).map_err(engine_error)?;
        let mut directory_locks = Vec::new();
        for disk in &config.disks {
            let directory = opened(PrivateDirectory::open_existing(&disk.root)
                .map_err(FreshBootstrapError::Filesystem)?)?;
            directory_locks.push(AdvisoryLock::directory(directory.directory())
                .map_err(FreshBootstrapError::Filesystem)?);
        }
        let selection = selection.select_fresh(&config, operation)?;
        let config = config.with_fresh_bootstrap(
            selection.fresh_selection().ok_or(FreshBootstrapError::Changed)?,
        ).map_err(engine_error)?;
        let resources = Arc::new(DiskResources::open(&config, false).map_err(engine_error)?);
        let admitted = selection.fresh_selection().ok_or(FreshBootstrapError::Changed)?;
        for (expected, disk) in admitted.disks.iter().zip(&config.disks) {
            let handle = resources.get(disk).map_err(engine_error)?;
            if expected.entity != LocalEntityIdentity::new(handle.seal.device, handle.seal.inode) {
                return Err(FreshBootstrapError::Changed);
            }
        }
        Ok(Self {
            root: root.to_owned(), config, resources, selection,
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
        let record = RootRecord {
            layout: StorageLayout::UnifiedWorkingTreeV1, selection, operation,
            system_writer, disk_writers, roots,
        };
        let bytes = record.bytes()?;
        AtomicFile::create(&system, &name(ROOTS)?, &bytes)
            .map_err(FreshBootstrapError::Filesystem)?;
        crate::engine::disk_lifecycle::bootstrap::runtime::initialize(
            &self.root, &self.config, &self.resources,
        ).map_err(engine_error)?;
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
        let engine = Engine::assemble_initialized(
            self.root, self.config, self.resources, locks, journals,
        ).map_err(engine_error)?;
        // Completion is last. An index write interrupted before this record is
        // not permission to rebuild or adopt that index during fresh retry.
        AtomicFile::create(&system, &name(COMPLETE)?, &bytes)
            .map_err(FreshBootstrapError::Filesystem)?;
        verify_record(&engine, &record)?;
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
fn verify_record(engine: &Engine, record: &RootRecord) -> Result<()> {
    verify_roots(engine.config(), record)
}
fn verify_roots(config: &ValidatedConfig, record: &RootRecord) -> Result<()> {
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
        let root = crate::disk::DiskRoot::open(&disk.root).map_err(engine_error)?;
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
pub(crate) fn fresh_root(engine: &Engine, id: RootId) -> crate::Result<AdmittedFreshRoot> {
    let inspect = || -> Result<AdmittedFreshRoot> {
        let system = PrivateDirectory::open_existing(&engine.root)
            .map_err(FreshBootstrapError::Filesystem)?;
        let selector = crate::authority_selection::FreshSelection::read(&system)?
            .ok_or(FreshBootstrapError::Incomplete)?;
        selector.require_directory(&system)?;
        selector.require_config(engine.config()).map_err(engine_error)?;
        let bytes = system.read_bounded(&name(ROOTS)?, MAX_BYTES)
            .map_err(FreshBootstrapError::Filesystem)?;
        let complete = system.read_bounded(&name(COMPLETE)?, MAX_BYTES)
            .map_err(FreshBootstrapError::Filesystem)?;
        if bytes != complete {
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
        verify_record(engine, &record)?;
        let binding = record.roots.into_iter().find(|binding| binding.root_id == id)
            .ok_or(FreshBootstrapError::Changed)?;
        Ok(AdmittedFreshRoot { binding })
    };
    inspect().map_err(failure)
}

impl Engine {
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
pub(crate) fn require_complete(root: &Path, config: &ValidatedConfig) -> crate::Result<()> {
    let inspect = || -> Result<()> {
        let system = PrivateDirectory::open_existing(root)
            .map_err(FreshBootstrapError::Filesystem)?;
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
        verify_roots(config, &record)?;
        system.verify_identity().map_err(FreshBootstrapError::Filesystem)
    };
    inspect().map_err(failure)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/fresh_directory_bootstrap_tests.rs"]
mod tests;
