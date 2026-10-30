//! Request-local branch metadata; no snapshot grants future admission.
use crate::{
    ApiError,
    backend_file_history::{BoundedWriter, EncodedPage},
};
use data_serialization::Serialize;
#[cfg(any(feature = "core", test))]
use data_serialization::{Serializer, ser::SerializeMap};
use std::collections::BTreeMap;
use storage_contract::{BranchKind, BranchState, ContentState};

#[derive(Clone, Debug)]
pub struct BranchSnapshot {
    pub branch_id: String,
    pub history_tree_id: String,
    pub name: String,
    pub head_commit_id: String,
    pub fork_commit_id: String,
    pub branch_kind: BranchKind,
    pub file_id: Option<String>,
    pub file_node_id: Option<String>,
    pub pending_file_versions: BTreeMap<String, String>,
    pub generation: u64,
    pub state: BranchState,
    pub file: Option<FileBranchSnapshot>,
}
#[derive(Clone, Debug)]
pub struct FileBranchSnapshot {
    pub content: ContentDispositionSnapshot,
    pub read_only: bool,
    pub alias_of: Option<String>,
    pub node_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct BranchResponse<S = String, P = BTreeMap<String, String>> {
    branch_id: S,
    history_tree_id: S,
    name: S,
    head_commit_id: S,
    fork_commit_id: S,
    branch_kind: BranchKind,
    #[serialization(skip_serializing_if = "Option::is_none")]
    file_id: Option<S>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    file_node_id: Option<S>,
    pending_file_versions: P,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    state: BranchState,
    materialized: bool,
    inherited: bool,
    content_state: &'static str,
    #[serialization(flatten)]
    file: Option<FileBranchResponse>,
}
#[derive(Serialize)]
struct FileBranchResponse {
    version_id: Option<String>,
    own_version_id: Option<String>,
    source_version_id: Option<String>,
    visible_content_state: &'static str,
    read_only: bool,
    alias_of: Option<String>,
    node_id: Option<String>,
}
impl From<BranchSnapshot> for BranchResponse {
    fn from(snapshot: BranchSnapshot) -> Self {
        let BranchSnapshot {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind,
            file_id,
            file_node_id,
            pending_file_versions,
            generation,
            state,
            file,
        } = snapshot;
        let materialized = pending_file_versions.is_empty();
        let inherited = !materialized;
        let content_state = if inherited { "PENDING" } else { "LIVE" };
        Self {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind,
            file_id,
            file_node_id,
            pending_file_versions,
            generation,
            state,
            materialized,
            inherited,
            content_state,
            file: None,
        }
        .with_file(file)
    }
}
impl<S, P> BranchResponse<S, P> {
    fn with_file(mut self, file: Option<FileBranchSnapshot>) -> Self {
        let file = file.map(|file| {
            let FileBranchSnapshot {
                content,
                read_only,
                alias_of,
                node_id,
            } = file;
            let ContentDispositionResponse {
                version_id,
                own_version_id,
                source_version_id,
                visible_content_state,
                materialized: file_materialized,
                inherited: file_inherited,
                content_state: file_state,
            } = content.into();
            self.materialized = file_materialized;
            self.inherited = file_inherited;
            self.content_state = file_state;
            FileBranchResponse {
                version_id,
                own_version_id,
                source_version_id,
                visible_content_state,
                read_only,
                alias_of,
                node_id,
            }
        });
        self.file = file;
        self
    }
}

/// Logical disposition at this admission, distinct from live replica health.
#[derive(Clone, Debug)]
pub struct ContentDispositionSnapshot {
    pub version_id: Option<String>,
    pub source_version_id: Option<String>,
    pub stored_state: Option<ContentState>,
}
#[derive(Debug, Serialize)]
pub(crate) struct ContentDispositionResponse {
    pub(crate) version_id: Option<String>,
    materialized: bool,
    inherited: bool,
    own_version_id: Option<String>,
    source_version_id: Option<String>,
    content_state: &'static str,
    visible_content_state: &'static str,
}
impl From<ContentDispositionSnapshot> for ContentDispositionResponse {
    fn from(snapshot: ContentDispositionSnapshot) -> Self {
        let ContentDispositionSnapshot {
            version_id,
            source_version_id,
            stored_state,
        } = snapshot;
        let inherited = source_version_id.is_some();
        let visible_content_state = stored_state.map(|state| state.as_str()).unwrap_or("LIVE");
        Self {
            own_version_id: (!inherited).then(|| version_id.clone()).flatten(),
            materialized: !inherited && version_id.is_some(),
            inherited,
            version_id,
            source_version_id,
            content_state: if inherited {
                "PENDING"
            } else {
                visible_content_state
            },
            visible_content_state,
        }
    }
}

/// Sorted head/branch overlay without copying keys or values.
#[cfg(any(feature = "core", test))]
pub(crate) struct PendingUnion<'a> {
    head: &'a BTreeMap<String, String>,
    branch: &'a BTreeMap<String, String>,
}
#[cfg(any(feature = "core", test))]
impl<'a> PendingUnion<'a> {
    pub(crate) fn new(
        head: &'a BTreeMap<String, String>,
        branch: &'a BTreeMap<String, String>,
    ) -> Self {
        Self { head, branch }
    }
    pub(crate) fn get(&self, file: &str) -> Option<&'a str> {
        self.branch
            .get(file)
            .or_else(|| self.head.get(file))
            .map(String::as_str)
    }
    fn is_empty(&self) -> bool {
        self.head.is_empty() && self.branch.is_empty()
    }
}
#[cfg(any(feature = "core", test))]
impl Serialize for PendingUnion<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_map(None)?;
        let mut head = self.head.iter().peekable();
        let mut branch = self.branch.iter().peekable();
        loop {
            let next = match (head.peek(), branch.peek()) {
                (None, None) => break,
                (Some(_), None) => head.next(),
                (None, Some(_)) => branch.next(),
                (Some((left, _)), Some((right, _))) => match left.cmp(right) {
                    std::cmp::Ordering::Less => head.next(),
                    std::cmp::Ordering::Greater => branch.next(),
                    std::cmp::Ordering::Equal => {
                        let _ = head.next();
                        branch.next()
                    }
                },
            };
            let (key, value) = next.expect("invariant: peeked pending iterator has its next entry");
            out.serialize_entry(key, value)?;
        }
        out.end()
    }
}
/// Small file facts accompany borrowed branch metadata and pending maps.
#[cfg(any(feature = "core", test))]
pub(crate) struct BorrowedBranch<'a> {
    pub(crate) branch: &'a storage_contract::Branch,
    pub(crate) pending: PendingUnion<'a>,
    pub(crate) file: Option<FileBranchSnapshot>,
}
#[cfg(any(feature = "core", test))]
impl<'a> From<BorrowedBranch<'a>> for BranchResponse<&'a str, PendingUnion<'a>> {
    fn from(row: BorrowedBranch<'a>) -> Self {
        let BorrowedBranch {
            branch,
            pending,
            file,
        } = row;
        let storage_contract::Branch {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind,
            file_id,
            file_node_id,
            pending_file_versions: _,
            generation,
            state,
        } = branch;
        let materialized = pending.is_empty();
        Self {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind: *branch_kind,
            file_id: file_id.as_deref(),
            file_node_id: file_node_id.as_deref(),
            pending_file_versions: pending,
            generation: *generation,
            state: *state,
            materialized,
            inherited: !materialized,
            content_state: if materialized { "LIVE" } else { "PENDING" },
            file: None,
        }
        .with_file(file)
    }
}

/// Complete single/list metadata response; owns bytes, not a publication or authority.
#[derive(Debug)]
pub struct BranchMetadataSnapshot {
    encoded: EncodedPage,
}
impl BranchMetadataSnapshot {
    /// Adapter encoder for the owned metadata supplied by non-engine backends.
    /// Complete responses above 8 MiB fail rather than truncate.
    pub fn encode(snapshot: BranchSnapshot) -> Result<Self, ApiError> {
        Self::encode_response(&BranchResponse::from(snapshot))
    }
    /// Preserve adapter list order; encode one complete bounded response.
    pub fn encode_list(
        snapshots: impl IntoIterator<Item = BranchSnapshot>,
    ) -> Result<Self, ApiError> {
        Self::encode_responses(snapshots.into_iter().map(BranchResponse::from))
    }
    #[cfg(any(feature = "core", test))]
    pub(crate) fn encode_borrowed(row: BorrowedBranch<'_>) -> Result<Self, ApiError> {
        Self::encode_response(&BranchResponse::from(row))
    }
    #[cfg(any(feature = "core", test))]
    pub(crate) fn encode_borrowed_list(rows: Vec<BorrowedBranch<'_>>) -> Result<Self, ApiError> {
        Self::encode_responses(rows.into_iter().map(BranchResponse::from))
    }
    fn encode_response(response: &impl Serialize) -> Result<Self, ApiError> {
        let mut writer = BoundedWriter::new(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
        writer.json(response)?;
        Ok(Self {
            encoded: writer.finish(),
        })
    }
    fn encode_responses<T: Serialize>(
        responses: impl IntoIterator<Item = T>,
    ) -> Result<Self, ApiError> {
        let mut writer = BoundedWriter::new(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
        writer.punctuation(b"{\"branches\":[")?;
        for (index, response) in responses.into_iter().enumerate() {
            if index != 0 {
                writer.punctuation(b",")?;
            }
            writer.json(&response)?;
        }
        writer.punctuation(b"]}")?;
        Ok(Self {
            encoded: writer.finish(),
        })
    }
    pub(crate) fn into_encoded(self) -> EncodedPage {
        self.encoded
    }
    #[cfg(test)]
    pub(crate) fn body(&self) -> &[u8] {
        self.encoded.body()
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_branch.rs"]
mod tests;
