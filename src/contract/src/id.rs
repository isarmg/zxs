//! Canonical identity text is decoded once into a domain-specific byte representation.
use crate::{Error, kind};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    str::FromStr,
};

mod node;
pub use node::FileNodeId;

/// The fixed wire prefix identifying one identity domain.
pub trait IdKind: 'static {
    const PREFIX: &'static str;
}

/// A domain-specific identity with canonical lowercase hexadecimal text.
pub struct Id<K: IdKind> {
    bytes: [u8; 16],
    kind: PhantomData<fn() -> K>,
}

impl<K: IdKind> Id<K> {
    /// Generate a UUID v4 identity directly in its domain representation.
    pub fn generate() -> crate::Result<Self> {
        Ok(Self::from_random(crate::random_uuid()?.into_bytes()))
    }

    /// Retain all bytes supplied by the caller's cryptographic random source.
    pub fn from_random(bytes: [u8; 16]) -> Self {
        Self {
            bytes,
            kind: PhantomData,
        }
    }
}

impl<K: IdKind> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: IdKind> Copy for Id<K> {}

impl<K: IdKind> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl<K: IdKind> Eq for Id<K> {}

impl<K: IdKind> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: IdKind> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.bytes.cmp(&other.bytes)
    }
}

impl<K: IdKind> Hash for Id<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl<K: IdKind> fmt::Debug for Id<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Id")
            .field(&format_args!("{self}"))
            .finish()
    }
}

impl<K: IdKind> FromStr for Id<K> {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::InvalidIdentifier {
            prefix: K::PREFIX,
            retryable: false,
        };
        let hex = value
            .strip_prefix(K::PREFIX)
            .and_then(|suffix| suffix.strip_prefix('_'))
            .ok_or_else(invalid)?;
        if hex.len() != 32
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid());
        }
        let mut bytes = [0; 16];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            let digit = |value: u8| {
                if value <= b'9' {
                    value - b'0'
                } else {
                    value - b'a' + 10
                }
            };
            *byte = digit(pair[0]) * 16 + digit(pair[1]);
        }
        Ok(Self::from_random(bytes))
    }
}

impl<K: IdKind> fmt::Display for Id<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}_{}",
            K::PREFIX,
            uuid::Uuid::from_bytes(self.bytes).simple()
        )
    }
}

impl<K: IdKind> Serialize for Id<K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de, K: IdKind> Deserialize<'de> for Id<K> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(data_serialization::de::Error::custom)
    }
}

pub type RuleId = Id<kind::Rule>;
pub type RepositoryId = Id<kind::Repository>;
pub type HistoryTreeId = Id<kind::HistoryTree>;
pub type BranchId = Id<kind::Branch>;
pub type CommitId = Id<kind::Commit>;
pub type TreeId = Id<kind::Tree>;
pub type DirectoryTreeId = Id<kind::DirectoryTree>;
pub type DirectoryBranchId = Id<kind::DirectoryBranch>;
pub type EntryId = Id<kind::Entry>;
pub type FileId = Id<kind::File>;
pub type VersionId = Id<kind::Version>;
pub type PoolId = Id<kind::Pool>;
pub type DiskId = Id<kind::Disk>;
pub type FileAnchorId = Id<kind::FileAnchor>;
pub type FileMetadataId = Id<kind::FileMetadata>;
pub type FileDeletionId = Id<kind::FileDeletion>;
pub type FilePathEventId = Id<kind::FilePathEvent>;
pub type FileMergeId = Id<kind::FileMerge>;

#[cfg(test)]
#[path = "../../../test/contract/unit/id.rs"]
mod tests;
