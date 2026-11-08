//! Read-only, same-history candidates, not persistent links or body availability.
use crate::history_window::{
    HistoryCollection, HistoryFileId, HistoryNodeId, HistoryPageLimit, HistoryRoot, HistoryTreeId,
    HistoryVersionId, HistoryWindowRequest,
};
use crate::{ContentHash, ContentState, Error, ErrorCode, Result};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_CONTENT_MATCHES: usize = 64;
pub const MAX_CONTENT_MATCH_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentMatchRequest {
    pub file_id: HistoryFileId,
    pub node_id: HistoryNodeId,
    pub history_root: HistoryRoot,
    pub after_version_id: Option<HistoryVersionId>,
    limit: usize,
}
impl ContentMatchRequest {
    pub fn parse(
        file: &str,
        node: &str,
        root: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Self> {
        if !(1..=MAX_CONTENT_MATCHES).contains(&limit) {
            return Err(invalid("content match limit must be between 1 and 64"));
        }
        Ok(Self {
            file_id: HistoryFileId::parse(file)?,
            node_id: HistoryNodeId::parse(node)?,
            history_root: HistoryRoot::parse(root)?,
            after_version_id: after.map(HistoryVersionId::parse).transpose()?,
            limit,
        })
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn route(&self) -> String {
        format!("/api/file-manager/files/{}/content-matches", self.file_id)
    }
    pub fn query_fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::from([
            ("node_id".into(), self.node_id.to_string()),
            ("history_root".into(), self.history_root.to_string()),
            ("limit".into(), self.limit.to_string()),
        ]);
        if let Some(after) = &self.after_version_id {
            fields.insert("after_version_id".into(), after.to_string());
        }
        fields
    }
    pub fn history_request(&self) -> HistoryWindowRequest {
        HistoryWindowRequest {
            file_id: self.file_id.clone(),
            collection: HistoryCollection::Nodes,
            location: None,
            history_root: Some(self.history_root),
            cursor: None,
            anchor_node_id: None,
            limit: HistoryPageLimit::for_collection(1, &HistoryCollection::Nodes)
                .expect("invariant: one node is within history policy"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContentMatch {
    pub creation_name: String,
    pub file_id: HistoryFileId,
    pub version_id: HistoryVersionId,
    #[serialization(deserialize_with = "required_option")]
    pub content_state: Option<ContentState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContentMatchPage {
    pub file_id: HistoryFileId,
    pub source_node_id: HistoryNodeId,
    pub source_version_id: HistoryVersionId,
    pub history_tree_id: HistoryTreeId,
    pub history_root: HistoryRoot,
    #[serialization(with = "crate::u64_string")]
    pub logical_size: u64,
    pub content_hash: ContentHash,
    #[serialization(deserialize_with = "required_option")]
    pub source_content_state: Option<ContentState>,
    pub matches: Vec<ContentMatch>,
    #[serialization(deserialize_with = "required_option")]
    pub next_after_version_id: Option<HistoryVersionId>,
}
impl ContentMatchPage {
    /// Validate an untrusted response before displaying or paginating it.
    pub fn validate(&self, request: &ContentMatchRequest) -> Result<()> {
        if self.file_id != request.file_id
            || self.source_node_id != request.node_id
            || self.history_root != request.history_root
            || (request.node_id.as_str().starts_with("ver_")
                && self.source_version_id.as_str() != request.node_id.as_str())
            || self.matches.len() > request.limit()
            || (self.logical_size == 0
                && (!self.matches.is_empty() || self.next_after_version_id.is_some()))
        {
            return Err(invalid(
                "content match response differs from its request or page bound",
            ));
        }
        let mut previous = request.after_version_id.as_ref();
        for found in &self.matches {
            name_policy::validate_name(&found.creation_name)?;
            if found.file_id == self.file_id
                || found.version_id == self.source_version_id
                || previous.is_some_and(|id| id >= &found.version_id)
            {
                return Err(invalid(
                    "content matches must be distinct, ordered versions of other files",
                ));
            }
            previous = Some(&found.version_id);
        }
        if let Some(next) = &self.next_after_version_id
            && (self.matches.len() != request.limit() || previous != Some(next))
        {
            return Err(invalid(
                "content match continuation must identify the last full-page result",
            ));
        }
        Ok(())
    }
}
fn required_option<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: data_serialization::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/content_matches.rs"]
mod tests;
