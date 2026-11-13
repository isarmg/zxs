//! Selection evidence only: callers retain all original directory locks. This
//! record never proves completion, nor authorizes adoption of partial material.
use crate::{ErrorCode, OperationId, ValidatedConfig};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use platform_io::fs_safety::{PayloadOpenMode, PrivateDirectory};
use std::{io::Write, path::PathBuf};
use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment};

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, error_definition::Error)]
pub enum ManualFormatError {
    #[error("manual format filesystem admission failed")]
    Filesystem(#[source] platform_io::fs_safety::Error),
    #[error("manual format metadata decoding failed")]
    Codec(#[source] storage_contract::Error),
    #[error("manual format requires independent primary level-one roots")]
    Topology,
    #[error("manual format requires empty roots; existing material was preserved")]
    Occupied,
    #[error("manual format original configuration, operation or root identity changed")]
    Changed,
    #[error("manual format metadata is unsupported, noncanonical or exceeds its budget")]
    Format,
    #[error("manual format initialization is incomplete")]
    Incomplete,
    #[error("this reader cannot admit manual container storage")]
    UnsupportedReader,
}
impl ManualFormatError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::Filesystem(_) | Self::Occupied | Self::Incomplete => ErrorCode::RecoveryRequired,
            Self::Changed => ErrorCode::PublicationInvalid,
            Self::Topology => ErrorCode::UnsupportedOperation,
            Self::Codec(_) | Self::Format | Self::UnsupportedReader => ErrorCode::FormatUnsupported,
        }
    }
}
type Result<T> = std::result::Result<T, ManualFormatError>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "fresh-manual-container-system-v1")]
    ManualContainer,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ManualDisk {
    pub(crate) path: PathBuf,
    pub(crate) entity: LocalEntityIdentity,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ManualSelection {
    format: Format,
    pub(crate) operation: OperationId,
    pub(crate) config: MaterialCommitment,
    pub(crate) system: LocalEntityIdentity,
    pub(crate) disks: Vec<ManualDisk>,
}
fn held(directory: &PrivateDirectory) -> Result<OpenAt2Root> {
    directory.verify_identity().map_err(ManualFormatError::Filesystem)?;
    OpenAt2Root::from_private_directory(
        directory.try_clone().map_err(ManualFormatError::Filesystem)?,
        MountPolicy::SameMount,
    ).map_err(ManualFormatError::Filesystem)
}
fn entity(root: &OpenAt2Root) -> LocalEntityIdentity {
    let identity = root.identity();
    LocalEntityIdentity::new(identity.device, identity.inode)
}
fn config_hash(config: &ValidatedConfig) -> Result<MaterialCommitment> {
    let bytes = storage_contract::canonical_json(config).map_err(ManualFormatError::Codec)?;
    let mut hash = blake3::Hasher::new_derive_key("zxs manual SYSTEM configuration v1");
    hash.update(&bytes);
    Ok(MaterialCommitment::from_digest(hash.finalize()))
}
fn require_empty(directory: &PrivateDirectory) -> Result<()> {
    if held(directory)?.entries().map_err(ManualFormatError::Filesystem)?
        .next().transpose().map_err(ManualFormatError::Filesystem)?.is_some()
    {
        return Err(ManualFormatError::Occupied);
    }
    Ok(())
}
impl ManualSelection {
    pub(crate) fn require_primary(config: &ValidatedConfig) -> Result<()> {
        if config.generation().is_some() || config.disks.is_empty()
            || config.disks.iter().any(|disk| disk.level != 1 || disk.upstream_slot_id.is_some())
        {
            return Err(ManualFormatError::Topology);
        }
        Ok(())
    }
    pub(crate) fn capture(
        system: &PrivateDirectory, config: &ValidatedConfig, operation: OperationId,
        directories: &[PrivateDirectory],
    ) -> Result<Self> {
        Self::require_primary(config)?;
        if directories.len() != config.disks.len() {
            return Err(ManualFormatError::Changed);
        }
        let system_entity = entity(&held(system)?);
        let mut disks: Vec<ManualDisk> = Vec::new();
        for (disk, directory) in config.disks.iter().zip(directories) {
            if directory.path() != disk.root {
                return Err(ManualFormatError::Changed);
            }
            let observed = entity(&held(directory)?);
            if observed == system_entity || disks.iter().any(|prior| prior.entity == observed)
                || disk.root.starts_with(system.path()) || system.path().starts_with(&disk.root)
                || disks.iter().any(|prior| {
                    prior.path.starts_with(&disk.root) || disk.root.starts_with(&prior.path)
                })
            {
                return Err(ManualFormatError::Changed);
            }
            disks.push(ManualDisk { path: disk.root.clone(), entity: observed });
        }
        let selection = Self {
            format: Format::ManualContainer, operation, config: config_hash(config)?,
            system: system_entity, disks,
        };
        selection.canonical_bytes()?;
        Ok(selection)
    }
    pub(crate) fn require_config(&self, config: &ValidatedConfig) -> Result<()> {
        Self::require_primary(config)?;
        if self.config != config_hash(config)? || self.disks.len() != config.disks.len()
            || self.disks.iter().zip(&config.disks).any(|(root, disk)| root.path != disk.root)
        {
            return Err(ManualFormatError::Changed);
        }
        Ok(())
    }
    pub(crate) fn require_held(
        &self, system: &PrivateDirectory, config: &ValidatedConfig,
        directories: &[PrivateDirectory],
    ) -> Result<()> {
        if &Self::capture(system, config, self.operation, directories)? != self {
            return Err(ManualFormatError::Changed);
        }
        Ok(())
    }
    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let bytes = storage_contract::canonical_json(self).map_err(ManualFormatError::Codec)?;
        if bytes.len() > MAX_BYTES {
            return Err(ManualFormatError::Format);
        }
        Ok(bytes)
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(ManualFormatError::Format);
        }
        let value: Self = storage_contract::decode_strict(bytes).map_err(ManualFormatError::Codec)?;
        if value.canonical_bytes()? != bytes || value.disks.is_empty() {
            return Err(ManualFormatError::Format);
        }
        for (index, disk) in value.disks.iter().enumerate() {
            if disk.entity == value.system || !disk.path.is_absolute()
                || value.disks[..index].iter().any(|prior| {
                    prior.entity == disk.entity || prior.path.starts_with(&disk.path)
                        || disk.path.starts_with(&prior.path)
                })
            {
                return Err(ManualFormatError::Format);
            }
        }
        Ok(value)
    }
    pub(crate) fn read(directory: &PrivateDirectory) -> Result<Option<Self>> {
        let name = &*crate::persist::layout::MANUAL_SYSTEM_SELECTION;
        match directory.read_bounded(name, MAX_BYTES) {
            Ok(bytes) => {
                directory.verify_identity().map_err(ManualFormatError::Filesystem)?;
                Self::decode(&bytes).map(Some)
            }
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {
                    directory.verify_identity().map_err(ManualFormatError::Filesystem)?;
                    Ok(None)
                }
            Err(error) => Err(ManualFormatError::Filesystem(error)),
        }
    }
    pub(crate) fn commitment(&self) -> Result<MaterialCommitment> {
        let mut hash = blake3::Hasher::new_derive_key("zxs manual SYSTEM selection v1");
        hash.update(&self.canonical_bytes()?);
        Ok(MaterialCommitment::from_digest(hash.finalize()))
    }
    pub(crate) fn commit_first_write(
        &self, system: &PrivateDirectory, config: &ValidatedConfig,
        directories: &[PrivateDirectory],
    ) -> Result<()> {
        self.require_held(system, config, directories)?;
        let bytes = self.canonical_bytes()?;
        require_empty(system)?;
        for directory in directories {
            require_empty(directory)?;
        }
        // Create the already-denied name, not an unrecognized atomic temporary.
        // A crash at any subsequent point leaves a refusal fence, never a hint
        // that an older writer may initialize or repair this root.
        let name = &*crate::persist::layout::MANUAL_SYSTEM_SELECTION;
        let mut file = system.open_payload(name, PayloadOpenMode::CreateNew)
            .map_err(ManualFormatError::Filesystem)?;
        system.sync().map_err(ManualFormatError::Filesystem)?;
        file.write_all(&bytes).map_err(|error| ManualFormatError::Filesystem(error.into()))?;
        file.sync_all().map_err(|error| ManualFormatError::Filesystem(error.into()))?;
        system.verify_payload(name, &file).map_err(ManualFormatError::Filesystem)?;
        system.sync().map_err(ManualFormatError::Filesystem)?;
        self.require_held(system, config, directories)?;
        if Self::read(system)?.as_ref() != Some(self) {
            return Err(ManualFormatError::Changed);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/manual_format_tests.rs"]
mod tests;
