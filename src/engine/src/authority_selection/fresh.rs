//! The first durable write in an explicitly fresh SYSTEM. Never a migration.
use super::SelectionGuard;
use crate::{ErrorCode, OperationId, ValidatedConfig};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, PayloadOpenMode, PrivateDirectory};
use std::io::Write;
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment};
use std::path::PathBuf;

// Already denied by the old writer before any lock sidecar is created.
pub(crate) const SELECTOR: &str = "initialized.json";
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, error_definition::Error)]
pub enum FreshBootstrapError {
    #[error("fresh bootstrap filesystem admission failed")]
    Filesystem(#[source] platform_io::fs_safety::Error),
    #[error("fresh bootstrap engine resource admission failed")]
    Engine(#[source] Box<crate::Error>),
    #[error("fresh bootstrap metadata encoding failed")]
    Codec(#[source] storage_contract::Error),
    #[error("this reader cannot admit the fresh directory snapshot disk format")]
    UnsupportedReader,
    #[error("fresh directory storage requires independent primary roots without upstreams")]
    Topology,
    #[error("fresh bootstrap directory is not empty; existing bytes were preserved")]
    Occupied,
    #[error("fresh bootstrap identity, configuration or original operation changed")]
    Changed,
    #[error("fresh bootstrap metadata is noncanonical, unsupported or exceeds its budget")]
    Format,
    #[error("fresh bootstrap has incomplete unclaimed material; explicit recovery is required")]
    Incomplete,
}
impl FreshBootstrapError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::Occupied | Self::Incomplete => ErrorCode::RecoveryRequired,
            Self::Changed => ErrorCode::PublicationInvalid,
            Self::Topology => ErrorCode::UnsupportedOperation,
            Self::Format | Self::Codec(_) | Self::UnsupportedReader => ErrorCode::FormatUnsupported,
            Self::Filesystem(_) | Self::Engine(_) => ErrorCode::RecoveryRequired,
        }
    }
}
type Result<T> = std::result::Result<T, FreshBootstrapError>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "unified-storage-system-v1")]
    UnifiedStorage,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FreshDisk {
    pub(crate) path: PathBuf,
    pub(crate) entity: LocalEntityIdentity,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FreshSelection {
    format: Format,
    pub(crate) operation: OperationId,
    pub(crate) config: MaterialCommitment,
    pub(crate) system: LocalEntityIdentity,
    pub(crate) disks: Vec<FreshDisk>,
}

pub(crate) fn held(directory: &PrivateDirectory) -> Result<OpenAt2Root> {
    OpenAt2Root::from_private_directory(
        directory.try_clone().map_err(FreshBootstrapError::Filesystem)?,
        MountPolicy::SameMount,
    ).map_err(FreshBootstrapError::Filesystem)
}
pub(crate) fn entity(root: &OpenAt2Root) -> LocalEntityIdentity {
    let id = root.identity();
    LocalEntityIdentity::new(id.device, id.inode)
}
fn config_hash(config: &ValidatedConfig) -> Result<MaterialCommitment> {
    let bytes = storage_contract::canonical_json(config).map_err(FreshBootstrapError::Codec)?;
    let mut hash = blake3::Hasher::new_derive_key("zxs unified storage SYSTEM configuration v1");
    hash.update(&bytes);
    Ok(MaterialCommitment::from_digest(hash.finalize()))
}
impl FreshSelection {
    pub(crate) fn require_primary(config: &ValidatedConfig) -> Result<()> {
        if config.disks.iter().any(|disk| disk.level != 1 || disk.upstream_slot_id.is_some()) {
            return Err(FreshBootstrapError::Topology);
        }
        Ok(())
    }
    fn capture(
        guard: &SelectionGuard, config: &ValidatedConfig, operation: OperationId,
        directories: &[PrivateDirectory],
    ) -> Result<Self> {
        Self::require_primary(config)?;
        let system = entity(&held(&guard.directory)?);
        let mut disks = Vec::new();
        if directories.len() != config.disks.len() {
            return Err(FreshBootstrapError::Changed);
        }
        for (disk, directory) in config.disks.iter().zip(directories) {
            if directory.path() != disk.root {
                return Err(FreshBootstrapError::Changed);
            }
            directory.verify_identity().map_err(FreshBootstrapError::Filesystem)?;
            let root = held(directory)?;
            let observed = entity(&root);
            if observed == system || disks.iter().any(|disk: &FreshDisk| disk.entity == observed) {
                return Err(FreshBootstrapError::Changed);
            }
            if disk.root.starts_with(guard.directory.path())
                || guard.directory.path().starts_with(&disk.root)
            {
                return Err(FreshBootstrapError::Changed);
            }
            disks.push(FreshDisk { path: disk.root.clone(), entity: observed });
        }
        Ok(Self {
            format: Format::UnifiedStorage,
            operation,
            config: config_hash(config)?,
            system,
            disks,
        })
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = storage_contract::canonical_json(self).map_err(FreshBootstrapError::Codec)?;
        if bytes.len() > MAX_BYTES {
            return Err(FreshBootstrapError::Format);
        }
        Ok(bytes)
    }
    pub(crate) fn read(directory: &PrivateDirectory) -> Result<Option<Self>> {
        let name = EntryName::new(SELECTOR).map_err(FreshBootstrapError::Filesystem)?;
        let bytes = match directory.read_bounded(&name, MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(FreshBootstrapError::Filesystem(error)),
        };
        let value: Self = storage_contract::decode_strict(&bytes)
            .map_err(FreshBootstrapError::Codec)?;
        if value.bytes()? != bytes {
            return Err(FreshBootstrapError::Format);
        }
        Ok(Some(value))
    }
}
impl SelectionGuard {
    pub(crate) fn select_fresh(
        self, config: &ValidatedConfig, operation: OperationId,
    ) -> Result<Self> {
        let directories = config.disks.iter().map(|disk| {
            PrivateDirectory::open_existing(&disk.root).map_err(FreshBootstrapError::Filesystem)
        }).collect::<Result<Vec<_>>>()?;
        self.select_fresh_held(config, operation, &directories)
    }
    pub(crate) fn select_fresh_held(
        mut self, config: &ValidatedConfig, operation: OperationId,
        directories: &[PrivateDirectory],
    ) -> Result<Self> {
        let expected = FreshSelection::capture(&self, config, operation, directories)?;
        if let Some(original) = FreshSelection::read(&self.directory)? {
            self.first_write = false;
            if original != expected {
                return Err(FreshBootstrapError::Changed);
            }
            let root = held(&self.directory)?;
            let mut only_selector = true;
            for entry in root.entries().map_err(FreshBootstrapError::Filesystem)? {
                if entry.map_err(FreshBootstrapError::Filesystem)?.as_os_str() != SELECTOR {
                    only_selector = false;
                }
            }
            for directory in directories {
                let root = held(directory)?;
                if root.entries().map_err(FreshBootstrapError::Filesystem)?
                    .next().transpose().map_err(FreshBootstrapError::Filesystem)?.is_some()
                {
                    only_selector = false;
                }
            }
            self.first_write = only_selector;
        } else {
            self.first_write = true;
            let root = held(&self.directory)?;
            if root.entries().map_err(FreshBootstrapError::Filesystem)?
                .next().transpose().map_err(FreshBootstrapError::Filesystem)?.is_some()
            {
                return Err(FreshBootstrapError::Occupied);
            }
            for (disk, directory) in expected.disks.iter().zip(directories) {
                let root = held(directory)?;
                if entity(&root) != disk.entity {
                    return Err(FreshBootstrapError::Changed);
                }
                if root.entries().map_err(FreshBootstrapError::Filesystem)?
                    .next().transpose().map_err(FreshBootstrapError::Filesystem)?.is_some()
                {
                    return Err(FreshBootstrapError::Occupied);
                }
            }
            // Publish the deny name itself before writing bytes. An atomic
            // staging tempfile would leave an old writer-unrecognized window.
            // A partial named selector is retained and rejected, never repaired.
            let bytes = expected.bytes()?;
            let name = EntryName::new(SELECTOR).map_err(FreshBootstrapError::Filesystem)?;
            let mut file = self.directory.open_payload(&name, PayloadOpenMode::CreateNew)
                .map_err(FreshBootstrapError::Filesystem)?;
            self.directory.sync().map_err(FreshBootstrapError::Filesystem)?;
            file.write_all(&bytes)
                .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
            file.sync_all()
                .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
            self.directory.verify_payload(&name, &file)
                .map_err(FreshBootstrapError::Filesystem)?;
            self.directory.sync().map_err(FreshBootstrapError::Filesystem)?;
        }
        if FreshSelection::read(&self.directory)?.as_ref() != Some(&expected) {
            return Err(FreshBootstrapError::Changed);
        }
        self.directory.sync().map_err(FreshBootstrapError::Filesystem)?;
        self.fresh = Some(expected);
        Ok(self)
    }
    pub(crate) fn recover_fresh(
        self, config: &ValidatedConfig,
    ) -> Result<Self> {
        let original = FreshSelection::read(&self.directory)?
            .ok_or(FreshBootstrapError::Incomplete)?;
        self.select_fresh(config, original.operation)
    }
    pub(crate) fn require_selected(&self) -> crate::Result<()> {
        if let Some(expected) = &self.fresh {
            let verify = || -> Result<()> {
                self.directory.verify_identity().map_err(FreshBootstrapError::Filesystem)?;
                if FreshSelection::read(&self.directory)?.as_ref() != Some(expected) {
                    return Err(FreshBootstrapError::Changed);
                }
                Ok(())
            };
            verify().map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))
        } else {
            self.require_legacy()
        }
    }
    pub(crate) fn fresh_selection(&self) -> Option<&FreshSelection> {
        self.fresh.as_ref()
    }
}

impl FreshSelection {
    pub(crate) fn require_directory(&self, directory: &PrivateDirectory) -> Result<()> {
        if entity(&held(directory)?) != self.system {
            return Err(FreshBootstrapError::Changed);
        }
        directory.verify_identity().map_err(FreshBootstrapError::Filesystem)
    }
    pub(crate) fn require_config(&self, config: &ValidatedConfig) -> crate::Result<()> {
        let checked = || -> Result<()> {
            Self::require_primary(config)?;
            if self.config != config_hash(config)? {
                return Err(FreshBootstrapError::Changed);
            }
            Ok(())
        };
        checked().map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))
    }
    pub(crate) fn commitment(&self) -> crate::Result<MaterialCommitment> {
        let bytes = self.bytes()
            .map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))?;
        let mut hash = blake3::Hasher::new_derive_key("zxs unified storage SYSTEM selection v1");
        hash.update(&bytes);
        Ok(MaterialCommitment::from_digest(hash.finalize()))
    }
}

impl SelectionGuard {
    pub(crate) fn selected_directory(&self) -> crate::Result<PrivateDirectory> {
        self.require_selected()?;
        self.directory.try_clone().map_err(crate::Error::from)
    }
}

pub(crate) fn has_fresh_selector(path: &std::path::Path) -> crate::Result<bool> {
    let inspect = || -> Result<bool> {
        let directory = PrivateDirectory::open_existing(path)
            .map_err(FreshBootstrapError::Filesystem)?;
        if FreshSelection::read(&directory)?.is_some() {
            return Ok(true);
        }
        for marker in [
            "fresh-directory-roots-v1.json", "fresh-directory-bootstrap-complete-v1.json",
        ] {
            match directory.read_bounded(
                &EntryName::new(marker).map_err(FreshBootstrapError::Filesystem)?, MAX_BYTES,
            ) {
                Ok(_) => return Err(FreshBootstrapError::Incomplete),
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(FreshBootstrapError::Filesystem(error)),
            }
        }
        Ok(false)
    };
    inspect().map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))
}

impl FreshSelection {
    /// Read-side closure for an independently held, seal-admitted local disk.
    /// This proves only the original bootstrap configuration, not a snapshot node.
    pub(crate) fn admit_disk(
        config: ValidatedConfig, seal: &crate::disk::DiskSeal, bytes: &[u8],
    ) -> crate::Result<ValidatedConfig> {
        let admit = || -> Result<Self> {
            Self::require_primary(&config)?;
            if bytes.len() > MAX_BYTES || config.generation().is_some() {
                return Err(FreshBootstrapError::Format);
            }
            let record: Self = storage_contract::decode_strict(bytes)
                .map_err(FreshBootstrapError::Codec)?;
            if record.bytes()? != bytes || record.config != config_hash(&config)?
                || record.disks.len() != config.disks.len()
            {
                return Err(FreshBootstrapError::Changed);
            }
            let crate::disk::DiskSealFormat::UnifiedStorage { bootstrap } = &seal.format
            else {
                return Err(FreshBootstrapError::Changed);
            };
            if record.commitment().map_err(|error| FreshBootstrapError::Engine(Box::new(error)))?
                != *bootstrap
            {
                return Err(FreshBootstrapError::Changed);
            }
            let mut found = false;
            for (root, disk) in record.disks.iter().zip(&config.disks) {
                if root.path != disk.root {
                    return Err(FreshBootstrapError::Changed);
                }
                if disk.disk_id == seal.disk_id {
                    if root.entity != LocalEntityIdentity::new(seal.device, seal.inode)
                        || seal.pool_id != config.pool_id || seal.slot_id != disk.slot_id
                        || seal.binding_generation != disk.binding_generation
                        || seal.simulation != (config.mode == storage_contract::Mode::Development)
                    {
                        return Err(FreshBootstrapError::Changed);
                    }
                    found = true;
                }
            }
            if !found {
                return Err(FreshBootstrapError::Changed);
            }
            Ok(record)
        };
        let selection = admit().map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))?;
        config.with_fresh_bootstrap(&selection)
    }
    pub(crate) fn canonical_bytes(&self) -> crate::Result<Vec<u8>> {
        self.bytes().map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))
    }
}
