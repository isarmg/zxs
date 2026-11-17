//! Bounded read projection of actual automatic node publications. IDs, cursors
//! and timestamps are filters, never credentials, leases or evidence of node creation.
use crate::snapshot_worktree::NamespaceDirectoryId;
use crate::{FileId, FileNodeId, OperationId, Principal, RepositoryId, Sequence, Timestamp};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

pub const PAGE_LIMIT: usize = 64;

#[derive(Debug, error_definition::Error)]
pub enum ObservationError {
    #[error("automatic publication interval must have start < end")]
    Interval,
    #[error("directory root does not belong to the requested repository")]
    Directory,
    #[error("automatic publication cursor does not match its account and observation")]
    Cursor,
    #[error("automatic publication page differs from the requested window")]
    Window,
    #[error(
        "automatic publication rows exceed their budget or are unordered, repeated, or outside the observation"
    )]
    Rows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Scope {
    pub repository: RepositoryId,
    pub directory: NamespaceDirectoryId,
    pub start: Timestamp,
    pub end: Timestamp,
}
impl Scope {
    pub fn validate(&self) -> Result<(), ObservationError> {
        if self.start >= self.end {
            return Err(ObservationError::Interval);
        }
        if matches!(self.directory, NamespaceDirectoryId::Repository(id) if id != self.repository) {
            return Err(ObservationError::Directory);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NodeKey {
    pub file: FileId,
    pub node: FileNodeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Position {
    First,
    After { observed: Sequence, last: NodeKey },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Request {
    pub scope: Scope,
    pub position: Position,
}
impl Request {
    pub fn validate(&self) -> Result<(), ObservationError> {
        self.scope.validate()?;
        if matches!(self.position, Position::After { observed, .. } if observed.get() == 0) {
            return Err(ObservationError::Cursor);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Row {
    pub key: NodeKey,
    pub operation: OperationId,
    pub sequence: Sequence,
    pub published_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Page {
    pub request: Request,
    pub observed: Sequence,
    #[serialization(deserialize_with = "bounded_rows")]
    pub rows: Vec<Row>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub next: Option<NodeKey>,
}
impl Page {
    pub fn validate(&self, requested: &Request) -> Result<(), ObservationError> {
        requested.validate()?;
        if self.request != *requested {
            return Err(ObservationError::Window);
        }
        let mut previous = match requested.position {
            Position::First => None,
            Position::After { observed, last } => {
                if observed != self.observed {
                    return Err(ObservationError::Window);
                }
                Some(last)
            }
        };
        if self.rows.len() > PAGE_LIMIT {
            return Err(ObservationError::Rows);
        }
        for row in &self.rows {
            if row.sequence.get() == 0
                || row.sequence > self.observed
                || row.published_at < requested.scope.start
                || row.published_at >= requested.scope.end
                || previous.is_some_and(|prior| prior >= row.key)
            {
                return Err(ObservationError::Rows);
            }
            previous = Some(row.key);
        }
        if self.next.is_some() && (self.rows.len() != PAGE_LIMIT || self.next != previous) {
            return Err(ObservationError::Cursor);
        }
        Ok(())
    }

    pub fn continuation(&self) -> Option<Request> {
        Some(Request {
            scope: self.request.scope,
            position: Position::After {
                observed: self.observed,
                last: self.next?,
            },
        })
    }
}

fn bounded_rows<'de, D: Deserializer<'de>>(input: D) -> Result<Vec<Row>, D::Error> {
    struct Rows;
    impl<'de> de::Visitor<'de> for Rows {
        type Value = Vec<Row>;
        fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(out, "at most {PAGE_LIMIT} automatic publication rows")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut items: A) -> Result<Vec<Row>, A::Error> {
            let mut rows = Vec::with_capacity(items.size_hint().unwrap_or(0).min(PAGE_LIMIT));
            while let Some(row) = items.next_element()? {
                if rows.len() == PAGE_LIMIT {
                    return Err(de::Error::custom(ObservationError::Rows));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    input.deserialize_seq(Rows)
}

/// Opaque continuation checksum, not an authorization token. The server supplies
/// its admitted session hash by borrow; that internal hash never goes on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding(blake3::Hash);
impl Binding {
    pub fn for_session(
        principal: &Principal,
        admitted_session: &str,
        request: &Request,
    ) -> crate::Result<Self> {
        let bytes = crate::canonical_json(&(principal, request))?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"automatic-file-nodes/original-window\0");
        hasher.update(&(admitted_session.len() as u64).to_be_bytes());
        hasher.update(admitted_session.as_bytes());
        hasher.update(&bytes);
        Ok(Self(hasher.finalize()))
    }
}
impl FromStr for Binding {
    type Err = ObservationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ObservationError::Cursor);
        }
        blake3::Hash::from_hex(value)
            .map(Self)
            .map_err(|_| ObservationError::Cursor)
    }
}
impl fmt::Display for Binding {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(self.0.to_hex().as_str())
    }
}
impl Serialize for Binding {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        out.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        String::deserialize(input)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// The next binding is issued only for the server's exact, authorized
/// continuation. End of the final page is explicit, never inferred from size.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Reply {
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub path: Option<name_policy::PortablePath>,
    pub page: Page,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub next_binding: Option<Binding>,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/automatic_observation.rs"]
mod tests;
