//! The current container name and immutable local version identity have separate roles.
use crate::{Error, Result, State};
use name_policy::Name;
use platform_io::fs_safety::EntryName;
use storage_contract::{ErrorCode, VersionId, VersionStorageId};

#[derive(Debug, error_definition::Error)]
pub enum VersionNameError {
    #[error("version filename rename differs from its retained inode-bound intent")]
    Intent,
    #[error("container has no current authoritative location")]
    MissingLocation,
    #[error("container name mirror differs from its current location")]
    Mirror,
    #[error("version storage identity collides with another retained version")]
    Collision,
    #[error("container or installed version name is invalid: {0}")]
    Name(#[source] name_policy::Error),
    #[error("container physical basename is absent or not UTF-8")]
    Basename,
    #[error("installed version filename exceeds its target component budget")]
    NameBudget,
    #[error("installed version entry is invalid: {0}")]
    Entry(#[source] platform_io::fs_safety::Error),
}

impl VersionNameError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::Collision => ErrorCode::NameCollision,
            Self::Intent | Self::MissingLocation | Self::Mirror | Self::Name(_) | Self::Entry(_)
            | Self::Basename | Self::NameBudget => {
                ErrorCode::ContainerLayoutInvalid
            }
        }
    }
}

pub(crate) fn current_container_name(state: &State, file: &str) -> Result<Name> {
    let location = state.file_locations.get(file)
        .ok_or(Error::VersionNames(VersionNameError::MissingLocation))?;
    let location = name_policy::PortablePath::parse(location)
        .map_err(|source| Error::VersionNames(VersionNameError::Name(source)))?;
    let name = location.file_name();
    if state.file_names.get(file).is_some_and(|mirror| mirror != name) {
        return Err(Error::VersionNames(VersionNameError::Mirror));
    }
    Name::parse(name).map_err(|source| Error::VersionNames(VersionNameError::Name(source)))
}

pub(crate) fn allocate_version_storage_id(
    state: &State,
    file: &str,
    version: VersionId,
) -> Result<VersionStorageId> {
    let id = VersionStorageId::from_version(version);
    if state.versions.values().any(|retained| {
        retained.file_id == file && retained.storage_id == id
    }) {
        return Err(Error::VersionNames(VersionNameError::Collision));
    }
    Ok(id)
}

pub(crate) fn installed_version_name(id: VersionStorageId, name: &Name) -> Result<String> {
    let leaf = format!("{id}-{name}");
    // The user component was admitted separately. Prefix bytes are checked
    // against the held target filesystem, not charged to its user-name policy.
    EntryName::new(&leaf)
        .map_err(|source| Error::VersionNames(VersionNameError::Entry(source)))?;
    Ok(leaf)
}

pub(crate) fn installed_version_leaf(id: VersionStorageId, name: &Name) -> Result<EntryName> {
    EntryName::new(installed_version_name(id, name)?)
        .map_err(|source| Error::VersionNames(VersionNameError::Entry(source)))
}

pub(crate) fn installed_version_path(
    container: &std::path::Path,
    version: &crate::FileVersion,
) -> Result<std::path::PathBuf> {
    let name = container.file_name().and_then(|name| name.to_str())
        .ok_or(Error::VersionNames(VersionNameError::Basename))?;
    let name = Name::parse(name)
        .map_err(|source| Error::VersionNames(VersionNameError::Name(source)))?;
    Ok(container.join("versions").join(installed_version_leaf(version.storage_id, &name)?.as_path()))
}

pub(super) fn check_target_name(root: &crate::disk::DiskRoot, leaf: &EntryName) -> Result<()> {
    if leaf.as_os_str().len() as u64 > root.component_name_maximum()? {
        return Err(Error::VersionNames(VersionNameError::NameBudget));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/version_names.rs"]
mod tests;
