//! Verified retained lifecycle facts, not a complete operation timeline.
use crate::history_window::{
    HistoryCollection, HistoryFileId, HistoryLocation, HistoryPageLimit, HistoryQueryHash,
    HistoryRepositoryId, HistoryResolvedContext, HistoryRoot, HistoryWindowRequest,
};
use crate::{Error, ErrorCode, FileDeletionId, FilePathEventId, OperationId, RepositoryId, Result};
pub use crate::Timestamp;
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::BTreeMap, fmt};

pub const MAX_AUDIT_EVENTS: usize = 65_536;
pub const MAX_AUDIT_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_EVENTS: usize = 64;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, error_definition::Error)]
pub enum LifecycleError {
    #[error("lifecycle page limit must be between 1 and 64")]
    Limit,
    #[error("lifecycle repository and path must be supplied together")]
    Location,
    #[error("invalid lifecycle cursor")]
    Cursor,
    #[error("lifecycle response differs from its request or event evidence")]
    Response,
    #[error("lifecycle audit or response exceeds its resource budget")]
    Bytes,
    #[error("lifecycle source has no complete retained audit evidence")]
    SourceIncomplete,
    #[error("lifecycle source changed; reload the first page")]
    Changed,
}
impl LifecycleError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Changed => ErrorCode::HistoryChanged,
            Self::SourceIncomplete => ErrorCode::PublicationInvalid,
            Self::Bytes => ErrorCode::ResourceLimit,
            Self::Limit | Self::Location | Self::Cursor | Self::Response => ErrorCode::InvalidRequest,
        }
    }
    pub fn into_error(self) -> Error {
        Error::FileLifecycle {
            source: self,
            retryable: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub file_id: HistoryFileId,
    pub history_root: HistoryRoot,
    pub location: Option<HistoryLocation>,
    pub cursor: Option<Cursor>,
    limit: usize,
}
impl Request {
    pub fn parse(
        file: &str,
        root: &str,
        repository: Option<&str>,
        path: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Self> {
        if !(1..=MAX_EVENTS).contains(&limit) {
            return Err(LifecycleError::Limit.into_error());
        }
        let location = match (repository, path) {
            (None, None) => None,
            (Some(repository), Some(path)) => Some(HistoryLocation::new(
                HistoryRepositoryId::parse(repository)?,
                name_policy::LookupPath::parse(path)?,
            )),
            _ => return Err(LifecycleError::Location.into_error()),
        };
        Ok(Self {
            file_id: HistoryFileId::parse(file)?,
            history_root: HistoryRoot::parse(root)?,
            location,
            cursor: cursor.map(Cursor::parse).transpose()?,
            limit,
        })
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn route(&self) -> String {
        format!("/api/file-manager/files/{}/history/lifecycle", self.file_id)
    }
    pub fn query_fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::from([
            ("history_root".into(), self.history_root.to_string()),
            ("limit".into(), self.limit.to_string()),
        ]);
        if let Some(location) = &self.location {
            fields.insert("repository_id".into(), location.repository_id.to_string());
            fields.insert("path".into(), location.path.as_str().to_owned());
        }
        if let Some(cursor) = &self.cursor {
            fields.insert("cursor".into(), cursor.to_string());
        }
        fields
    }
    pub fn history_request(&self) -> HistoryWindowRequest {
        HistoryWindowRequest {
            file_id: self.file_id.clone(),
            collection: HistoryCollection::Nodes,
            location: self.location.clone(),
            history_root: Some(self.history_root),
            cursor: None,
            anchor_node_id: None,
            limit: HistoryPageLimit::for_collection(1, &HistoryCollection::Nodes)
                .expect("invariant: one node is within history policy"),
        }
    }
    pub fn binding(&self, context: &HistoryResolvedContext, sequence: u64) -> HistoryQueryHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"file-lifecycle-path-facts\0");
        let context_hash = self.history_request().query_hash(context, None);
        hasher.update(context_hash.to_string().as_bytes());
        hasher.update(self.history_root.to_string().as_bytes());
        hasher.update(&sequence.to_le_bytes());
        hasher.update(&(self.limit as u64).to_le_bytes());
        HistoryQueryHash::from_digest(hasher.finalize())
    }
}

/// A query-bound continuation, never an authority token or historical-root lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub binding: HistoryQueryHash,
    pub after_operation_id: OperationId,
    pub after_sequence: u64,
    pub after_source_id: SourceId,
}
impl Cursor {
    pub fn parse(text: &str) -> Result<Self> {
        if !(140..=160).contains(&text.len()) {
            return Err(LifecycleError::Cursor.into_error());
        }
        let (binding, operation) = text
            .split_once(':')
            .ok_or_else(|| LifecycleError::Cursor.into_error())?;
        let (operation, sequence) = operation
            .split_once(':')
            .ok_or_else(|| LifecycleError::Cursor.into_error())?;
        let (sequence, source) = sequence
            .split_once(':')
            .ok_or_else(|| LifecycleError::Cursor.into_error())?;
        let after_sequence = crate::parse_u64_decimal(sequence)?;
        if after_sequence == 0 {
            return Err(LifecycleError::Cursor.into_error());
        }
        Ok(Self {
            binding: HistoryQueryHash::parse(binding)?,
            after_operation_id: operation.parse()?,
            after_sequence,
            after_source_id: SourceId::parse(source)?,
        })
    }
    pub fn after_key(&self) -> (u64, OperationId, SourceId) {
        (self.after_sequence, self.after_operation_id, self.after_source_id)
    }
}
impl fmt::Display for Cursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}:{}",
            self.binding,
            self.after_operation_id,
            self.after_sequence,
            self.after_source_id,
        )
    }
}
impl Serialize for Cursor {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::parse(&String::deserialize(d)?).map_err(data_serialization::de::Error::custom)
    }
}

/// The actual retained record identity, never a position in a projected page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceId {
    Deletion(FileDeletionId),
    PathChange(FilePathEventId),
}
impl SourceId {
    pub fn parse(text: &str) -> Result<Self> {
        if text.starts_with("fdel_") {
            text.parse().map(Self::Deletion)
        } else if text.starts_with("fpath_") {
            text.parse().map(Self::PathChange)
        } else {
            Err(LifecycleError::Cursor.into_error())
        }
    }
}
impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deletion(id) => fmt::Display::fmt(id, formatter),
            Self::PathChange(id) => fmt::Display::fmt(id, formatter),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PathLocation {
    pub repository_id: RepositoryId,
    pub path: name_policy::PortablePath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Detail {
    Delete {
        deletion_id: FileDeletionId,
    },
    Undo {
        deletion_id: FileDeletionId,
    },
    Import {
        deletion_id: FileDeletionId,
    },
    PathChanged {
        event_id: FilePathEventId,
        before: PathLocation,
        after: PathLocation,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Event {
    pub operation_id: OperationId,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub detail: Detail,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub occurred_at: Option<Timestamp>,
}
impl Event {
    pub fn source_id(&self) -> SourceId {
        match &self.detail {
            Detail::Delete { deletion_id }
            | Detail::Undo { deletion_id }
            | Detail::Import { deletion_id } => SourceId::Deletion(*deletion_id),
            Detail::PathChanged { event_id, .. } => SourceId::PathChange(*event_id),
        }
    }
    pub fn order_key(&self) -> (u64, OperationId, SourceId) {
        (self.sequence, self.operation_id, self.source_id())
    }
    fn has_valid_evidence_shape(&self) -> bool {
        match &self.detail {
            Detail::Delete { .. } => self.occurred_at.is_some(),
            Detail::Undo { .. } | Detail::Import { .. } => self.occurred_at.is_none(),
            Detail::PathChanged { before, after, .. } => {
                self.occurred_at.is_some() && before != after
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Page {
    pub file_id: HistoryFileId,
    pub history_root: HistoryRoot,
    pub context: HistoryResolvedContext,
    #[serialization(with = "crate::u64_string")]
    pub observation_sequence: u64,
    pub events: Vec<Event>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub next_cursor: Option<Cursor>,
}
impl Page {
    fn validate_encoded_budget(&self) -> Result<()> {
        // Count the same compact JSON as the response without allocating another page.
        // Key order changes no encoded length; escaped and multibyte text still count.
        struct Limit {
            remaining: usize,
            exceeded: bool,
        }
        impl std::io::Write for Limit {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > self.remaining {
                    self.exceeded = true;
                    return Err(std::io::Error::other("lifecycle response exceeds its byte limit"));
                }
                self.remaining -= bytes.len();
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut limit = Limit {
            remaining: MAX_RESPONSE_BYTES,
            exceeded: false,
        };
        match json_codec::to_writer(&mut limit, self) {
            Ok(()) => Ok(()),
            Err(_) if limit.exceeded => Err(LifecycleError::Bytes.into_error()),
            Err(source) => Err(Error::Json {
                source,
                retryable: false,
            }),
        }
    }

    pub fn validate(&self, request: &Request) -> Result<()> {
        let binding = request.binding(&self.context, self.observation_sequence);
        if self.file_id != request.file_id
            || self.history_root != request.history_root
            || self.events.len() > request.limit()
            || request.cursor.as_ref().is_some_and(|cursor| {
                cursor.binding != binding
                    || cursor.after_sequence == 0
                    || cursor.after_sequence > self.observation_sequence
            })
            || request.location.as_ref().is_some_and(|location| {
                location.repository_id != self.context.repository_id
                    || self.context.path.as_ref().map(name_policy::PortablePath::key)
                        != Some(location.path.key())
            })
        {
            return Err(LifecycleError::Response.into_error());
        }
        let mut previous = request.cursor.as_ref().map(Cursor::after_key);
        let mut members = std::collections::BTreeSet::new();
        for event in &self.events {
            let key = event.order_key();
            if event.sequence == 0
                || previous.is_some_and(|previous| key <= previous)
                || event.sequence > self.observation_sequence
                || !members.insert((event.operation_id, event.source_id()))
                || request.cursor.as_ref().is_some_and(|cursor| {
                    cursor.after_operation_id == event.operation_id
                        && cursor.after_source_id == event.source_id()
                })
                || !event.has_valid_evidence_shape()
            {
                return Err(LifecycleError::Response.into_error());
            }
            previous = Some(key);
        }
        if let Some(cursor) = &self.next_cursor
            && (cursor.binding != binding
                || self.events.last().map(Event::order_key) != Some(cursor.after_key()))
        {
            return Err(LifecycleError::Response.into_error());
        }
        self.validate_encoded_budget()
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/file_lifecycle.rs"]
mod tests;
