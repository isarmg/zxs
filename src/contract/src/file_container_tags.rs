//! File identity owns its current tags independently of retained content nodes.
use crate::{FileId, Generation, RepositoryId};
use data_serialization::{Deserialize, Deserializer, Serialize};
use name_policy::PortablePath;
use std::collections::BTreeSet;

pub const KIND: &str = "file.container-tags.replace";
pub use crate::text::{MAX_TAG_BYTES, MAX_FILE_TAGS as MAX_TAGS};

#[derive(Clone, Debug, PartialEq, Eq, error_definition::Error)]
pub enum TagsError {
    #[error("tag must be nonempty, bounded and contain no edge whitespace or controls")]
    InvalidTag,
    #[error("tag set exceeds sixteen items")]
    Limit,
    #[error("tag set contains a duplicate")]
    Duplicate,
    #[error("container tag response differs from its request")]
    Response,
    #[error("file container tag authority is missing")]
    MissingRecord,
    #[error("container tag snapshot has another key domain")]
    SnapshotKeyDomain,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serialization(transparent)]
pub struct Tag(String);

impl TagsError {
    pub fn code(&self) -> crate::ErrorCode {
        match self {
            Self::InvalidTag | Self::Duplicate => crate::ErrorCode::InvalidTag,
            Self::Limit => crate::ErrorCode::TagLimitExceeded,
            Self::Response | Self::MissingRecord | Self::SnapshotKeyDomain => {
                crate::ErrorCode::StateInvalid
            }
        }
    }
    pub fn into_error(self) -> crate::Error {
        crate::Error::ContainerTags {
            source: self,
            retryable: false,
        }
    }
}

impl Tag {
    pub fn parse(value: String) -> Result<Self, TagsError> {
        if value.is_empty()
            || value.len() > MAX_TAG_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(TagsError::InvalidTag);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?)
            .map_err(data_serialization::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serialization(transparent)]
pub struct TagSet(BTreeSet<Tag>);

impl TagSet {
    pub fn new(values: impl IntoIterator<Item = Tag>) -> Result<Self, TagsError> {
        let mut tags = BTreeSet::new();
        for tag in values {
            if tags.len() == MAX_TAGS {
                return Err(TagsError::Limit);
            }
            if !tags.insert(tag) {
                return Err(TagsError::Duplicate);
            }
        }
        Ok(Self(tags))
    }
    pub fn empty() -> Self {
        Self(BTreeSet::new())
    }
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Tag> {
        self.0.iter()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for TagSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> data_serialization::de::Visitor<'de> for Visitor {
            type Value = TagSet;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("at most sixteen distinct tags")
            }
            fn visit_seq<A: data_serialization::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<TagSet, A::Error> {
                let mut tags = BTreeSet::new();
                while let Some(tag) = sequence.next_element::<Tag>()? {
                    if tags.len() == MAX_TAGS {
                        return Err(data_serialization::de::Error::custom(TagsError::Limit));
                    }
                    if !tags.insert(tag) {
                        return Err(data_serialization::de::Error::custom(TagsError::Duplicate));
                    }
                }
                Ok(TagSet(tags))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContainerTags {
    tags: TagSet,
    generation: Generation,
}

impl ContainerTags {
    pub fn new(tags: TagSet, generation: Generation) -> Self {
        Self { tags, generation }
    }
    pub fn empty() -> Self {
        Self::new(TagSet::empty(), Generation::new(0))
    }
    pub fn tags(&self) -> &TagSet {
        &self.tags
    }
    pub fn generation(&self) -> Generation {
        self.generation
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Context {
    pub repository_id: RepositoryId,
    pub path: PortablePath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadRequest {
    pub file_id: FileId,
    pub context: Context,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Snapshot {
    pub file_id: FileId,
    pub context: Context,
    pub tags: TagSet,
    pub generation: Generation,
}

impl Snapshot {
    pub fn validate(&self, request: &ReadRequest) -> Result<(), TagsError> {
        if self.file_id != request.file_id || self.context != request.context {
            return Err(TagsError::Response);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReplaceCommand {
    pub file_id: FileId,
    pub context: Context,
    pub expected_generation: Generation,
    pub tags: TagSet,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Replacement {
    pub request: ReplaceCommand,
    pub tags: TagSet,
    pub generation: Generation,
    pub changed: bool,
}

impl Replacement {
    pub fn validate(&self, command: &ReplaceCommand) -> Result<(), TagsError> {
        let expected = if self.changed {
            command
                .expected_generation
                .next()
                .map_err(|_| TagsError::Response)?
        } else {
            command.expected_generation
        };
        if &self.request != command || self.tags != command.tags || self.generation != expected {
            return Err(TagsError::Response);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/file_container_tags.rs"]
mod tests;
