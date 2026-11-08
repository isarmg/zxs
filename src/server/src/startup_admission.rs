//! No-create preparation gate for the normal ZXS executable. This is a local
//! initial-setup gate, not a disk-health certificate or replacement for Engine's
//! authenticated configuration, terminal selection and storage admission.
use crate::{ApiError, setup_recovery::SetupRecovery};
use platform_io::fs_safety::{EntryName, Error as FilesystemError, PrivateDirectory};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const MAX_CONFIGURATION_BYTES: usize = 64 * 1024 * 1024;
const INITIALIZATION: &str = "management-initialization.json";
const AUTHORITY: &str = "management-authority.json";

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error(
        "normal ZXS requires an existing safe private directory {path:?}; use the independent zxs-helper for first preparation or recovery, not an empty replacement directory"
    )]
    Directory {
        path: PathBuf,
        #[source]
        source: FilesystemError,
    },
    #[error(
        "cannot read existing preparation evidence {name} in {path:?}; preserve it and use zxs-helper, no fresh initialization was attempted"
    )]
    Evidence {
        path: PathBuf,
        name: &'static str,
        #[source]
        source: FilesystemError,
    },
    #[error(
        "normal ZXS cannot start without an existing storage configuration; finish first storage initialization or recover the original system with zxs-helper; no directory or account was created"
    )]
    StorageNotPrepared,
    #[error(
        "initial management authority is missing or differs from the current format; preserve all files and resolve the original operation with zxs-helper"
    )]
    InitialAuthority,
    #[error(
        "initial storage activation is not durably complete; use the original zxs-helper plan before starting normal ZXS"
    )]
    Activation {
        #[source]
        source: ApiError,
    },
    #[error("protected lifecycle selection cannot be read; this is not an empty system")]
    Lifecycle {
        #[source]
        source: storage_engine::Error,
    },
}

/// Keep the existing directory capabilities through runtime teardown. They
/// never create or chmod paths and are not a lock against external root admin.
/// The normal entry also owns its host-wide ServiceLease for this whole period.
pub struct PreparedService {
    system: PrivateDirectory,
    management: PrivateDirectory,
}
impl PreparedService {
    /// Read only SYSTEM and management directory metadata. No candidate/data
    /// disk, Engine, credential writer, bootstrap or socket is opened here.
    ///
    /// # Errors
    /// Missing/unsafe private roots, absent storage configuration, incomplete
    /// initial activation, or malformed current lifecycle evidence are retained
    /// as distinct failures. Existing authentication or disk faults are still
    /// diagnosed by the normal management/Engine owners, never reset here.
    pub fn open(system: &Path, management: &Path) -> Result<Self, Error> {
        let system = open_directory(system)?;
        let management = open_directory(management)?;
        let prepared = Self { system, management };
        prepared.recheck()?;
        // Select a terminal or interrupted membership owner before activation
        // proof could consult a superseded configuration. Only SYSTEM is read;
        // this is not a claim that the eventual terminal authority is healthy.
        if storage_engine::TerminalManagement::requires_management(prepared.system.path())
            .map_err(|source| Error::Lifecycle { source })?
        {
            prepared.recheck()?;
            return Ok(prepared);
        }
        if read_optional(&prepared.system, "config.json", MAX_CONFIGURATION_BYTES)?.is_none() {
            return Err(Error::StorageNotPrepared);
        }
        if let Some(intent) = read_optional(&prepared.system, INITIALIZATION, 4096)? {
            if intent != crate::setup_recovery::management_initialization_record()
                || read_optional(&prepared.system, AUTHORITY, 4096)?.as_deref()
                    != Some(crate::management::MANAGEMENT_AUTHORITY_BYTES)
            {
                return Err(Error::InitialAuthority);
            }
        }
        // Existing recovered systems without an initial helper activation
        // intent retain their current core recovery semantics. This does not
        // accept any legacy format or invent missing credentials/readiness.
        let setup = SetupRecovery::new(
            prepared.system.path().to_owned(),
            prepared.management.path().to_owned(),
            BTreeMap::new(),
        )
        .map_err(|source| Error::Activation { source })?;
        setup
            .activation_admitted()
            .map_err(|source| Error::Activation { source })?;
        prepared.recheck()?;
        Ok(prepared)
    }

    pub(crate) fn roots(&self) -> (&Path, &Path) {
        (self.system.path(), self.management.path())
    }
    pub(crate) fn credential_directory(&self) -> Result<PrivateDirectory, Error> {
        self.recheck()?;
        self.management
            .try_clone()
            .map_err(|source| Error::Directory {
                path: self.management.path().to_owned(),
                source,
            })
    }
    pub(crate) fn system_directory(&self) -> Result<PrivateDirectory, Error> {
        self.recheck()?;
        self.system.try_clone().map_err(|source| Error::Directory {
            path: self.system.path().to_owned(),
            source,
        })
    }
    pub(crate) fn require_current_authority(&self) -> Result<(), Error> {
        self.recheck()?;
        require_existing_authority(&self.system)
    }

    /// Revalidate captured names at process use/teardown boundaries. Holding an
    /// old inode alone is never evidence that a later mount/path stayed bound.
    ///
    /// # Errors
    /// Returns the concrete unsafe/rebound directory failure without accepting
    /// a new path as the earlier prepared installation.
    pub fn recheck(&self) -> Result<(), Error> {
        for directory in [&self.system, &self.management] {
            directory
                .verify_identity()
                .map_err(|source| Error::Directory {
                    path: directory.path().to_owned(),
                    source,
                })?;
        }
        Ok(())
    }
}
/// A normal recovery attempt may re-read the marker, never recreate it. Keep
/// the original SYSTEM directory capability so a new path cannot inherit it.
pub(crate) fn require_existing_authority(directory: &PrivateDirectory) -> Result<(), Error> {
    directory
        .verify_identity()
        .map_err(|source| Error::Directory {
            path: directory.path().to_owned(),
            source,
        })?;
    if read_optional(directory, AUTHORITY, 4096)?.as_deref()
        != Some(crate::management::MANAGEMENT_AUTHORITY_BYTES)
    {
        return Err(Error::InitialAuthority);
    }
    Ok(())
}

fn open_directory(path: &Path) -> Result<PrivateDirectory, Error> {
    PrivateDirectory::open_existing(path).map_err(|source| Error::Directory {
        path: path.to_owned(),
        source,
    })
}
fn read_optional(
    directory: &PrivateDirectory,
    name: &'static str,
    maximum: usize,
) -> Result<Option<Vec<u8>>, Error> {
    let name_entry =
        EntryName::new(name).expect("invariant: preparation names are fixed single components");
    match directory.read_bounded(&name_entry, maximum) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(FilesystemError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Evidence {
            path: directory.path().to_owned(),
            name,
            source,
        }),
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/startup_admission_tests.rs"]
mod tests;
