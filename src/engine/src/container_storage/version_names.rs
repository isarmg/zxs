//! The current container name and immutable local version identity have separate roles.
use crate::{Error, Result, State};
use name_policy::Name;
use platform_io::fs_safety::EntryName;
use storage_contract::{ErrorCode, VersionId, VersionStorageId};

#[derive(Debug, error_definition::Error)]
pub enum VersionNameError {
    #[error("immutable version material differs from its held original occurrence")]
    Material,
    #[error("checking retained BODY files exceeds the shared descriptor budget")]
    Descriptors,
    #[error("container has no current authoritative location")]
    MissingLocation,
    #[error("container name mirror differs from its current location")]
    Mirror,
    #[error("version storage identity collides with another retained version")]
    Collision,
    #[error("container or installed version name is invalid: {0}")]
    Name(#[source] name_policy::Error),
    #[error("installed version filename exceeds its target component budget")]
    NameBudget,
    #[error("installed version entry is invalid: {0}")]
    Entry(#[source] platform_io::fs_safety::Error),
}

impl VersionNameError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::Collision => ErrorCode::NameCollision,
            Self::Descriptors => ErrorCode::ResourceLimit,
            Self::Material | Self::MissingLocation | Self::Mirror | Self::Name(_) | Self::Entry(_)
            | Self::NameBudget => {
                ErrorCode::ContainerLayoutInvalid
            }
        }
    }
}

pub(crate) fn current_container_name(state: &State, file: &str) -> Result<Name> {
    container_name_from_locations(&state.file_locations, &state.file_names, file)
}

pub(crate) fn container_name_from_locations(
    locations: &storage_contract::TrackedMap<String>,
    names: &storage_contract::TrackedMap<String>,
    file: &str,
) -> Result<Name> {
    let location = locations.get(file)
        .ok_or(Error::VersionNames(VersionNameError::MissingLocation))?;
    let location = name_policy::PortablePath::parse(location)
        .map_err(|source| Error::VersionNames(VersionNameError::Name(source)))?;
    let name = location.file_name();
    if names.get(file).is_some_and(|mirror| mirror != name) {
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

/// Only an owned Version has an installed BODY. A current container name,
/// selected node, archive prefix or download name cannot select another leaf.
pub(crate) fn installed_version_leaf(version: &crate::FileVersion) -> Result<EntryName> {
    let name = Name::parse(&version.creation_name)
        .map_err(|source| Error::VersionNames(VersionNameError::Name(source)))?;
    EntryName::new(format!("{}-{name}", version.storage_id))
        .map_err(|source| Error::VersionNames(VersionNameError::Entry(source)))
}

pub(crate) fn installed_version_name(version: &crate::FileVersion) -> Result<String> {
    let leaf = installed_version_leaf(version)?;
    Ok(leaf.as_os_str().to_str()
        .expect("invariant: the version leaf is formed exclusively from UTF-8").into())
}

pub(crate) fn installed_version_path(
    container: &std::path::Path,
    version: &crate::FileVersion,
) -> Result<std::path::PathBuf> {
    Ok(container.join(crate::persist::layout::VERSIONS.as_path())
        .join(installed_version_leaf(version)?.as_path()))
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
