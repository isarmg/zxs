//! Explicit new-root format selection and completion evidence. Not migration.
use crate::authority_selection::manual::{ManualFormatError, ManualSelection};
use crate::authority_selection::SelectionGuard;
use crate::engine::container_drafts::format_error;
use crate::persist::layout;
use crate::{Config, Engine, OperationId, Result, ValidatedConfig};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root};
use platform_io::fs_safety::{PayloadOpenMode, PrivateDirectory};
use std::{io::Write, path::Path, sync::Arc};
use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment};

const MAX_BYTES: usize = 1024 * 1024;

/// Only a completed fresh-format admission authorizes this new persisted state.
/// A plain Config, an old initializer, or a selector alone is not that proof.
pub(crate) fn require_manual_storage(config: &ValidatedConfig) -> Result<()> {
    if config.manual_selection().is_none() || config.manual_completion().is_none() {
        return Err(format_error(ManualFormatError::UnsupportedReader));
    }
    Ok(())
}

/// Rebind a decoded saved Config only to the original completed SYSTEM proof.
/// Neither the decoded Config nor the DISK selector can create that permission.
pub(crate) fn admit_saved_genesis(
    saved: ValidatedConfig,
    projected: &ValidatedConfig,
    seal: &crate::disk::DiskSeal,
    selector_bytes: &[u8],
) -> Result<ValidatedConfig> {
    let selection = projected.manual_selection()
        .ok_or_else(|| format_error(ManualFormatError::UnsupportedReader))?;
    let completion = projected.manual_completion()
        .ok_or_else(|| format_error(ManualFormatError::UnsupportedReader))?;
    if selector_bytes != selection.canonical_bytes().map_err(format_error)? {
        return Err(format_error(ManualFormatError::Changed));
    }
    selection.require_config(&saved).map_err(format_error)?;
    admit_local(saved, seal, selector_bytes)?.with_manual_completion(completion.clone())
}

fn identity(value: FileIdentity) -> LocalEntityIdentity {
    LocalEntityIdentity::new(value.device, value.inode)
}

fn opened(directory: PrivateDirectory) -> Result<OpenAt2Root> {
    Ok(OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)?)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Complete {
    selection: MaterialCommitment,
    system_writer: LocalEntityIdentity,
    bootstrap_root: LocalEntityIdentity,
    bootstrap_writer: LocalEntityIdentity,
    disk_writers: Vec<LocalEntityIdentity>,
}

fn bootstrap_identity(
    system: &PrivateDirectory,
) -> Result<(LocalEntityIdentity, LocalEntityIdentity)> {
    let name = platform_io::fs_safety::EntryName::new(
        crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY,
    )?;
    let system_root = opened(system.try_clone()?)?;
    let directory = system_root.open_directory(&name.as_relative())?;
    let root_identity = identity(FileIdentity::from_fd(&directory)?);
    let directory = PrivateDirectory::from_directory(
        directory, system.path().join(name.as_path()),
    )?;
    let writer = directory.open_payload(&layout::CONTROL_WRITER_LOCK, PayloadOpenMode::Existing)?;
    if writer.metadata()?.len() != 0 {
        return Err(format_error(ManualFormatError::Changed));
    }
    Ok((root_identity, identity(FileIdentity::from_fd(&writer)?)))
}

fn complete_commitment(proof: &Complete) -> Result<MaterialCommitment> {
    let bytes = storage_contract::canonical_json(proof)?;
    let mut hash = blake3::Hasher::new_derive_key("zxs manual initialization completion v1");
    hash.update(&bytes);
    Ok(MaterialCommitment::from_digest(hash.finalize()))
}

pub(crate) fn admit_bootstrap_handle(
    system: &PrivateDirectory,
    config: &ValidatedConfig,
    bootstrap: FileIdentity,
) -> Result<LocalEntityIdentity> {
    let selection = config.manual_selection()
        .ok_or_else(|| format_error(ManualFormatError::Changed))?;
    let proof = complete(system)?;
    if config.manual_completion() != Some(&complete_commitment(&proof)?)
        || proof.selection != selection.commitment().map_err(format_error)?
        || identity(opened(system.try_clone()?)?.identity()) != selection.system
        || ManualSelection::read(system).map_err(format_error)?.as_ref() != Some(selection)
        || identity(bootstrap) != proof.bootstrap_root
    {
        return Err(format_error(ManualFormatError::Changed));
    }
    Ok(proof.bootstrap_writer)
}

fn complete(system: &PrivateDirectory) -> Result<Complete> {
    let bytes = system.read_bounded(&layout::MANUAL_INITIALIZATION_COMPLETE, MAX_BYTES)
        .map_err(|error| match error {
            platform_io::fs_safety::Error::Io(error)
                if error.kind() == std::io::ErrorKind::NotFound => {
                    format_error(ManualFormatError::Incomplete)
                }
            other => other.into(),
        })?;
    let value: Complete = storage_contract::decode_strict(&bytes)?;
    if storage_contract::canonical_json(&value)? != bytes {
        return Err(format_error(ManualFormatError::Format));
    }
    Ok(value)
}

/// Check original roots, selectors and existing writer identities before any
/// legacy recovery path could create a missing sidecar or repair a projection.
pub(crate) fn require_complete(
    system: &PrivateDirectory,
    config: &ValidatedConfig,
    selection: &ManualSelection,
) -> Result<MaterialCommitment> {
    let disks = config.disks.iter().map(|disk| PrivateDirectory::open_existing(&disk.root))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    selection.require_held(system, config, &disks).map_err(format_error)?;
    let proof = complete(system)?;
    let commitment = complete_commitment(&proof)?;
    if config.manual_completion().is_some_and(|expected| expected != &commitment) {
        return Err(format_error(ManualFormatError::Changed));
    }
    if proof.selection != selection.commitment().map_err(format_error)?
        || proof.disk_writers.len() != disks.len()
    {
        return Err(format_error(ManualFormatError::Changed));
    }
    let (bootstrap_root, bootstrap_writer) = bootstrap_identity(system)?;
    if bootstrap_root != proof.bootstrap_root || bootstrap_writer != proof.bootstrap_writer {
        return Err(format_error(ManualFormatError::Changed));
    }
    let writer = system.open_payload(&layout::CONTROL_WRITER_LOCK, PayloadOpenMode::Existing)?;
    if identity(FileIdentity::from_fd(&writer)?) != proof.system_writer
        || writer.metadata()?.len() != 0
    {
        return Err(format_error(ManualFormatError::Changed));
    }
    let expected = selection.canonical_bytes().map_err(format_error)?;
    for (index, directory) in disks.iter().enumerate() {
        if directory.read_bounded(&layout::MANUAL_DISK_SELECTION, MAX_BYTES)? != expected {
            return Err(format_error(ManualFormatError::Changed));
        }
        let writer = directory.open_payload(&layout::DISK_WRITER_LOCK, PayloadOpenMode::Existing)?;
        if identity(FileIdentity::from_fd(&writer)?) != proof.disk_writers[index]
            || writer.metadata()?.len() != 0
        {
            return Err(format_error(ManualFormatError::Changed));
        }
        let seal = crate::disk::decode_seal(
            &directory.read_bounded(&layout::DISK_IDENTITY, MAX_BYTES)?,
        )?;
        let admitted = admit_local(config.clone(), &seal, &expected)?;
        let root = opened(directory.try_clone()?)?;
        if crate::disk::identity_at(&admitted, &config.disks[index], root.directory())? != seal {
            return Err(format_error(ManualFormatError::Changed));
        }
    }
    Ok(commitment)
}

pub(crate) fn admit_local(
    config: ValidatedConfig,
    seal: &crate::disk::DiskSeal,
    bytes: &[u8],
) -> Result<ValidatedConfig> {
    let selection = ManualSelection::decode(bytes).map_err(format_error)?;
    selection.require_config(&config).map_err(format_error)?;
    let commitment = selection.commitment().map_err(format_error)?;
    if seal.format != (crate::disk::DiskSealFormat::ManualContainerDrafts {
        selection: commitment,
    }) {
        return Err(format_error(ManualFormatError::Changed));
    }
    let index = config.disks.iter().position(|disk| {
        disk.disk_id == seal.disk_id && disk.slot_id == seal.slot_id
            && disk.binding_generation == seal.binding_generation
    }).ok_or_else(|| format_error(ManualFormatError::Changed))?;
    if selection.disks[index].entity != LocalEntityIdentity::new(seal.device, seal.inode)
        || selection.disks[index].path != config.disks[index].root
    {
        return Err(format_error(ManualFormatError::Changed));
    }
    config.with_manual_selection(&selection)
}

impl Engine {
    /// Only explicitly selected, existing empty SYSTEM and primary directories.
    /// Old/unknown/partial material is never upgraded, repaired or removed here.
    /// The first selection/install admission retains original descriptors; this
    /// does not claim all inherited SQLite/locator recovery I/O is FD-relative.
    pub fn initialize_manual_container_storage(
        root: &Path,
        config: Config,
        operation: OperationId,
    ) -> Result<Self> {
        let config = ValidatedConfig::try_from(config)?;
        ManualSelection::require_primary(&config).map_err(format_error)?;
        let system = PrivateDirectory::open_existing(root)?;
        let root = system.path().to_owned();
        let selection_guard = SelectionGuard::acquire_held(system)?;
        let system = selection_guard.selected_directory()?;
        let directories = config.disks.iter()
            .map(|disk| PrivateDirectory::open_existing(&disk.root))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut locks = Vec::new();
        for directory in &directories {
            locks.push(AdvisoryLock::directory(opened(directory.try_clone()?)?.directory())?);
        }
        let selection = ManualSelection::capture(&system, &config, operation, &directories)
            .map_err(format_error)?;
        if let Some(original) = ManualSelection::read(&system).map_err(format_error)? {
            if original != selection {
                return Err(format_error(ManualFormatError::Changed));
            }
            require_complete(&system, &config, &selection)?;
            drop(locks);
            drop(selection_guard);
            return Self::open(&root);
        }
        selection.commit_first_write(&system, &config, &directories).map_err(format_error)?;
        let config = config.with_manual_selection(&selection)?;
        let handles = directories.into_iter().map(|directory| {
            Ok(opened(directory)?.into_directory())
        }).collect::<Result<Vec<_>>>()?;
        let resources = Arc::new(crate::disk::DiskResources::initialize_held(&config, handles)?);
        let mut engine = Self::initialize_resources_locked(
            root, config, resources, selection_guard.with_manual(selection.clone()), &locks,
        )?;
        let bytes = selection.canonical_bytes().map_err(format_error)?;
        let mut disk_writers = Vec::new();
        for disk in &engine.config.disks {
            let held = engine.disk_resources.get(disk)?;
            engine._locks.verify_disk(disk, &held.root)?;
            let directory = held.root.private_directory(held.root.path())?;
            if directory.read_bounded(&layout::MANUAL_DISK_SELECTION, MAX_BYTES)? != bytes {
                return Err(format_error(ManualFormatError::Changed));
            }
            let writer = directory.open_payload(
                &layout::DISK_WRITER_LOCK, PayloadOpenMode::Existing,
            )?;
            disk_writers.push(identity(FileIdentity::from_fd(&writer)?));
        }
        let (bootstrap_root, bootstrap_writer) = bootstrap_identity(&system)?;
        let proof = Complete {
            bootstrap_root,
            bootstrap_writer,
            selection: selection.commitment().map_err(format_error)?,
            system_writer: identity(engine._locks.system_identity()?),
            disk_writers,
        };
        let mut file = system.open_payload(&layout::MANUAL_INITIALIZATION_COMPLETE,
            PayloadOpenMode::CreateNew)?;
        file.write_all(&storage_contract::canonical_json(&proof)?)?;
        file.sync_all()?;
        system.verify_payload(&layout::MANUAL_INITIALIZATION_COMPLETE, &file)?;
        system.sync()?;
        let completion = require_complete(&system, &engine.config, &selection)?;
        engine.config = engine.config.clone().with_manual_completion(completion)?;
        Ok(engine)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/manual_container_initialization_tests.rs"]
mod tests;
