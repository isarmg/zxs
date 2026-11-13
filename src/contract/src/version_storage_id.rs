//! Stable local version-file identity, independent of a container's current name.
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VersionStorageId([u8; 15]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
#[error("version storage identity requires exactly fifteen lowercase ASCII letters or digits")]
pub struct InvalidVersionStorageId;

/// Invalid derived storage leaf received across a wire boundary.
#[derive(Debug, error_definition::Error)]
pub enum InvalidInstalledVersionName {
    #[error("installed version leaf lacks its identity separator")]
    Separator,
    #[error("installed version leaf has an invalid identity")]
    Identity(#[source] InvalidVersionStorageId),
    #[error("installed version leaf has an invalid current name")]
    Name(#[source] name_policy::Error),
    #[error("installed version leaf differs from the current download name")]
    CurrentName,
}

/// Validate a read projection, not an independent filename authority. Only the
/// suffix spends the user-name budget; filesystem component admission remains
/// the responsibility of the installer holding the target root.
pub fn validate_installed_version_name(
    stored: &str,
    download: &str,
) -> Result<(), InvalidInstalledVersionName> {
    let (identity, name) = stored.split_once('-')
        .ok_or(InvalidInstalledVersionName::Separator)?;
    identity.parse::<VersionStorageId>()
        .map_err(InvalidInstalledVersionName::Identity)?;
    name_policy::validate_name(name).map_err(InvalidInstalledVersionName::Name)?;
    if name != download {
        return Err(InvalidInstalledVersionName::CurrentName);
    }
    Ok(())
}

impl VersionStorageId {
    /// A short identity is not a uniqueness proof. The allocating container
    /// must reject collisions before committing this version.
    pub fn from_version(version: crate::VersionId) -> Self {
        let mut hash = blake3::Hasher::new();
        hash.update(b"zxs.version-storage-identity\0");
        hash.update(version.to_string().as_bytes());
        let encoded = hash.finalize().to_hex();
        let mut bytes = [0; 15];
        bytes.copy_from_slice(&encoded.as_bytes()[..15]);
        Self(bytes)
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0)
            .expect("invariant: storage identity constructors retain only ASCII")
    }
}

impl FromStr for VersionStorageId {
    type Err = InvalidVersionStorageId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 15
            || !value.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        {
            return Err(InvalidVersionStorageId);
        }
        let mut bytes = [0; 15];
        bytes.copy_from_slice(value.as_bytes());
        Ok(Self(bytes))
    }
}

impl fmt::Display for VersionStorageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for VersionStorageId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for VersionStorageId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(data_serialization::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/version_storage_id.rs"]
mod tests;
