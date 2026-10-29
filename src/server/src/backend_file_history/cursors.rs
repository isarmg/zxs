//! Typed continuation cursors: strict JSON encoded as lowercase hexadecimal.
//!
//! A cursor is a non-authorizing position description. It binds the exact
//! history root, the file and history identities, the normalized request
//! fingerprint and an exclusive boundary key. Decoding re-parses every field;
//! the backend compares root and fingerprint before using the key.
use super::limits::{MAX_CURSOR_CHARS, MAX_ID_BYTES};
use crate::ApiError;
use data_serialization::{Deserialize, Serialize};

const VERSION: u8 = 1;

fn invalid_cursor(message: &'static str) -> ApiError {
    ApiError::new(
        ::http::StatusCode::BAD_REQUEST,
        storage_contract::ErrorCode::InvalidCursor,
        message,
    )
}

pub(crate) fn decode<C: data_serialization::de::DeserializeOwned>(
    value: &str,
) -> Result<C, ApiError> {
    if value.is_empty() || value.len() > MAX_CURSOR_CHARS || !value.len().is_multiple_of(2) {
        return Err(invalid_cursor("invalid history cursor length"));
    }
    let bytes = value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(invalid_cursor("invalid history cursor encoding")),
            };
            Ok((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect::<Result<Vec<u8>, ApiError>>()?;
    json_codec::from_slice(&bytes)
        .map_err(|source| invalid_cursor("invalid history cursor").with_source(source))
}

pub(crate) fn encode<C: Serialize>(cursor: &C) -> Result<String, ApiError> {
    let bytes =
        json_codec::to_vec(cursor).expect("invariant: internal JSON serialization succeeds");
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("invariant: formatting into String succeeds");
    }
    if encoded.len() > MAX_CURSOR_CHARS {
        return Err(ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::StateInvalid,
            "history cursor exceeds its bounded representation",
        ));
    }
    Ok(encoded)
}

fn check_version(v: u8) -> Result<(), ApiError> {
    if v != VERSION {
        return Err(invalid_cursor("unsupported history cursor version"));
    }
    Ok(())
}

fn check_id(field: &'static str, value: &str) -> Result<(), ApiError> {
    if value.is_empty() || value.len() > MAX_ID_BYTES {
        return Err(invalid_cursor(match field {
            "file_id" => "invalid history cursor file identity",
            "history_id" => "invalid history cursor history identity",
            "node_id" => "invalid history cursor node identity",
            _ => "invalid history cursor identity",
        }));
    }
    Ok(())
}

/// Shared binding of every cursor kind. Produced only by the backend after
/// the request was admitted and the root captured; decoded from the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryCursorSeed {
    pub v: u8,
    pub kind: CursorKind,
    pub root: String,
    pub history_id: String,
    pub file_id: String,
    /// blake3 hex of the normalized query (context, limit, order, budget id).
    pub request: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum CursorKind {
    Nodes,
    Parents,
    Branches,
    Merges,
    Versions,
}

impl HistoryCursorSeed {
    pub fn new(
        kind: CursorKind,
        root: &str,
        history_id: &str,
        file_id: &str,
        request: &json_codec::Value,
    ) -> Self {
        let bytes =
            json_codec::to_vec(request).expect("invariant: internal JSON serialization succeeds");
        Self {
            v: VERSION,
            kind,
            root: root.to_owned(),
            history_id: history_id.to_owned(),
            file_id: file_id.to_owned(),
            request: blake3::hash(&bytes).to_hex().to_string(),
        }
    }
    fn validate(&self, kind: CursorKind) -> Result<(), ApiError> {
        check_version(self.v)?;
        if self.kind != kind {
            return Err(invalid_cursor(
                "history cursor belongs to another collection",
            ));
        }
        super::HistoryRootText::parse(&self.root)
            .map_err(|_| invalid_cursor("invalid history cursor root"))?;
        check_id("history_id", &self.history_id)?;
        check_id("file_id", &self.file_id)?;
        if self.request.len() != 64
            || !self
                .request
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid_cursor("invalid history cursor request digest"));
        }
        Ok(())
    }
    /// A decoded cursor continues the same query only when its fingerprint
    /// and scope match the freshly computed ones. Root comparison is separate
    /// (409), after admission.
    pub fn admit(&self, expected: &Self) -> Result<(), ApiError> {
        self.admit_scope(expected)?;
        if self.request != expected.request {
            return Err(ApiError::invalid("cursor does not match the history query"));
        }
        Ok(())
    }
    /// Scope-only admission for cursors minted by another page shape (the
    /// parent previews of a node page continue through the parents route).
    pub fn admit_scope(&self, expected: &Self) -> Result<(), ApiError> {
        if self.history_id != expected.history_id
            || self.file_id != expected.file_id
            || self.kind != expected.kind
        {
            return Err(ApiError::invalid("cursor does not match the history query"));
        }
        Ok(())
    }
}

/// A decoded continuation cursor of one collection kind.
pub trait HistoryCursor: data_serialization::de::DeserializeOwned {
    fn seed(&self) -> &HistoryCursorSeed;
    fn validate(&self) -> Result<(), ApiError>;
}

/// Decode a cursor at the request boundary: strict format, every field
/// re-parsed, and bound to the file named by the request path. A cursor
/// minted for another file is a malformed request (400), never a stale
/// page (409), and is rejected before any State lookup.
pub(crate) fn decode_bound<C: HistoryCursor>(value: &str, file_id: &str) -> Result<C, ApiError> {
    let cursor: C = decode(value)?;
    cursor.validate()?;
    if cursor.seed().file_id != file_id {
        return Err(ApiError::invalid("cursor does not match the history query"));
    }
    Ok(cursor)
}

macro_rules! cursor {
    ($name:ident, $kind:expr, $key:ty, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serialization(deny_unknown_fields)]
        pub struct $name {
            #[serialization(flatten)]
            pub seed: HistoryCursorSeed,
            pub backward: bool,
            pub key: $key,
        }
        impl $name {
            pub fn root(&self) -> &str {
                &self.seed.root
            }
            pub fn encode(
                seed: &HistoryCursorSeed,
                backward: bool,
                key: $key,
            ) -> Result<String, ApiError> {
                encode(&Self {
                    seed: seed.clone(),
                    backward,
                    key,
                })
            }
            /// Re-parse every field after decoding, before any State lookup.
            pub fn validate(&self) -> Result<(), ApiError> {
                self.seed.validate($kind)?;
                self.key.validate()
            }
        }
        impl HistoryCursor for $name {
            fn seed(&self) -> &HistoryCursorSeed {
                &self.seed
            }
            fn validate(&self) -> Result<(), ApiError> {
                $name::validate(self)
            }
        }
    };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NodeCursorKey {
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub node_id: String,
}
impl NodeCursorKey {
    fn validate(&self) -> Result<(), ApiError> {
        check_id("node_id", &self.node_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ParentCursorKey {
    pub node_id: String,
    /// Exclusive lower bound on parent ids; None restarts from the first parent
    /// (minted for a node that received no preview budget).
    pub after: Option<String>,
}
impl ParentCursorKey {
    fn validate(&self) -> Result<(), ApiError> {
        check_id("node_id", &self.node_id)?;
        match &self.after {
            Some(after) => check_id("after", after),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BranchCursorKey {
    pub after: String,
}
impl BranchCursorKey {
    fn validate(&self) -> Result<(), ApiError> {
        check_id("after", &self.after)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct MergeCursorKey {
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub merge_id: String,
}
impl MergeCursorKey {
    fn validate(&self) -> Result<(), ApiError> {
        check_id("merge_id", &self.merge_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct VersionCursorKey {
    #[serialization(with = "storage_contract::u64_string")]
    pub version_sequence: u64,
    pub version_id: String,
}
impl VersionCursorKey {
    fn validate(&self) -> Result<(), ApiError> {
        check_id("version_id", &self.version_id)
    }
}

cursor!(
    NodeCursor,
    CursorKind::Nodes,
    NodeCursorKey,
    "Node page boundary: (sequence DESC, node_id ASC), exclusive."
);
cursor!(
    ParentCursor,
    CursorKind::Parents,
    ParentCursorKey,
    "Parent page boundary of one node: parent id ASC, exclusive."
);
cursor!(
    BranchCursor,
    CursorKind::Branches,
    BranchCursorKey,
    "Branch page boundary: branch id ASC, exclusive."
);
cursor!(
    MergeCursor,
    CursorKind::Merges,
    MergeCursorKey,
    "Merge page boundary: (sequence DESC, merge_id ASC), exclusive."
);
cursor!(
    VersionCursor,
    CursorKind::Versions,
    VersionCursorKey,
    "Body version page boundary: (version_sequence ASC, version_id ASC), exclusive."
);

#[cfg(test)]
#[path = "../../../../test/server/unit/backend_file_history/cursor_tests.rs"]
mod tests;
