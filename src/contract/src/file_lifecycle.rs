//! Verified retained lifecycle facts, not a complete operation timeline.
use crate::history_window::{
    HistoryCollection, HistoryFileId, HistoryLocation, HistoryPageLimit, HistoryQueryHash,
    HistoryRepositoryId, HistoryResolvedContext, HistoryRoot, HistoryWindowRequest,
};
use crate::{Error, ErrorCode, FileDeletionId, OperationId, Result};
pub use crate::Timestamp;
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::BTreeMap, fmt};

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
        hasher.update(b"file-lifecycle-retained-v1\0");
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
}
impl Cursor {
    pub fn parse(text: &str) -> Result<Self> {
        if !(102..=121).contains(&text.len()) {
            return Err(LifecycleError::Cursor.into_error());
        }
        let (binding, operation) = text
            .split_once(':')
            .ok_or_else(|| LifecycleError::Cursor.into_error())?;
        let (operation, sequence) = operation
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
        })
    }
}
impl fmt::Display for Cursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.binding,
            self.after_operation_id,
            self.after_sequence,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum Action {
    Delete,
    Undo,
    Import,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Event {
    pub operation_id: OperationId,
    pub action: Action,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub deletion_id: FileDeletionId,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub occurred_at: Option<Timestamp>,
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
        let mut previous = request
            .cursor
            .as_ref()
            .map_or(0, |cursor| cursor.after_sequence);
        let mut operations = std::collections::BTreeSet::new();
        for event in &self.events {
            if event.sequence <= previous
                || event.sequence > self.observation_sequence
                || !operations.insert(event.operation_id)
                || (event.action == Action::Delete) != event.occurred_at.is_some()
                || request
                    .cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.after_operation_id == event.operation_id)
            {
                return Err(LifecycleError::Response.into_error());
            }
            previous = event.sequence;
        }
        if let Some(cursor) = &self.next_cursor
            && (cursor.binding != binding
                || self.events.len() != request.limit()
                || self.events.last().map(|event| event.operation_id)
                    != Some(cursor.after_operation_id)
                || self.events.last().map(|event| event.sequence) != Some(cursor.after_sequence))
        {
            return Err(LifecycleError::Response.into_error());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/file_lifecycle.rs"]
mod tests;
