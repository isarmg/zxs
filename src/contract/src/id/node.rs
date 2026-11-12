use crate::{Error, Id, IdKind, kind};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{cmp::Ordering, fmt, str::FromStr};

/// File graph identities; each consuming boundary selects its supported variants.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FileNodeId {
    Version(Id<kind::Version>),
    Anchor(Id<kind::FileAnchor>),
    Metadata(Id<kind::FileMetadata>),
    Merge(Id<kind::FileMerge>),
}

impl FileNodeId {
    fn ordering_key(&self) -> (&'static str, &[u8; 16]) {
        match self {
            Self::Version(id) => (kind::Version::PREFIX, &id.bytes),
            Self::Anchor(id) => (kind::FileAnchor::PREFIX, &id.bytes),
            Self::Metadata(id) => (kind::FileMetadata::PREFIX, &id.bytes),
            Self::Merge(id) => (kind::FileMerge::PREFIX, &id.bytes),
        }
    }
}

impl PartialOrd for FileNodeId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileNodeId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ordering_key().cmp(&other.ordering_key())
    }
}

impl FromStr for FileNodeId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.split_once('_') {
            Some((kind::Version::PREFIX, _)) => value.parse().map(Self::Version),
            Some((kind::FileAnchor::PREFIX, _)) => value.parse().map(Self::Anchor),
            Some((kind::FileMetadata::PREFIX, _)) => value.parse().map(Self::Metadata),
            Some((kind::FileMerge::PREFIX, _)) => value.parse().map(Self::Merge),
            _ => Err(Error::InvalidIdentifier {
                prefix: "file node",
                retryable: false,
            }),
        }
    }
}

impl fmt::Display for FileNodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(id) => fmt::Display::fmt(id, formatter),
            Self::Anchor(id) => fmt::Display::fmt(id, formatter),
            Self::Metadata(id) => fmt::Display::fmt(id, formatter),
            Self::Merge(id) => fmt::Display::fmt(id, formatter),
        }
    }
}

impl Serialize for FileNodeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FileNodeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(data_serialization::de::Error::custom)
    }
}
