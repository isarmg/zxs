use crate::api;
use data_serialization::{Deserialize, Deserializer};
use json_codec::Value;
use std::{collections::BTreeSet, sync::Arc};
use storage_contract::{
    FileDeletionNode,
    client_protocol::MAX_FILE_DELETION_FRONTIER_ENTRIES,
    history_window::{
        HistoryBranchId, HistoryFileId, HistoryNodeId, HistoryRepositoryId, HistoryTreeId,
    },
};

#[derive(Clone, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Summary {
    pub file_id: HistoryFileId,
    pub deletion_node_id: String,
    pub repository_id: HistoryRepositoryId,
    pub path: name_policy::PortablePath,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Page {
    pub snapshot: String,
    pub entries: Vec<Summary>,
    #[serialization(deserialize_with = "nullable")]
    pub next_after: Option<HistoryFileId>,
}
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<HistoryFileId>, D::Error> {
    Option::deserialize(d)
}
fn error(source: json_codec::Error) -> api::Error {
    api::Error {
        status: 200,
        code: storage_contract::ErrorCode::InvalidResponse,
        message: "The file deletion response did not match its request or contract.".into(),
        retryable: false,
        request_id: None,
        source: Some(Arc::new(api::ErrorSource::FileDeletion(source))),
    }
}
fn invalid(message: &str) -> api::Error {
    error(<json_codec::Error as data_serialization::de::Error>::custom(message))
}
pub(super) fn page(value: Value) -> Result<Page, api::Error> {
    let page = Page::deserialize(value).map_err(error)?;
    if page.snapshot.is_empty()
        || page.snapshot.len() > 256
        || page.entries.len() > 100
        || page
            .entries
            .windows(2)
            .any(|w| w[0].file_id >= w[1].file_id)
        || page
            .entries
            .iter()
            .any(|entry| storage_contract::validate_id(&entry.deletion_node_id, "fdel").is_err())
        || page.next_after.as_ref().is_some_and(|after| {
            page.entries.len() != 100
                || page
                    .entries
                    .last()
                    .is_none_or(|entry| &entry.file_id != after)
        })
    {
        return Err(invalid("invalid deletion page order, bounds or cursor"));
    }
    Ok(page)
}
pub(super) fn node(
    value: Value,
    summary: &Summary,
) -> Result<Option<FileDeletionNode>, api::Error> {
    let node = Option::<FileDeletionNode>::deserialize(value).map_err(error)?;
    let Some(node) = node else {
        return Ok(None);
    };
    if node.file_id != summary.file_id.as_str()
        || node.repository_id != summary.repository_id.as_str()
        || node.node_id != summary.deletion_node_id
        || node.path != summary.path
        || HistoryTreeId::parse(&node.history_tree_id).is_err()
        || node.branch_heads.is_empty()
        || node.branch_heads.len() > MAX_FILE_DELETION_FRONTIER_ENTRIES
        || node.parent_node_ids.len() > MAX_FILE_DELETION_FRONTIER_ENTRIES
        || node.branch_heads.iter().any(|(branch, head)| {
            HistoryBranchId::parse(branch).is_err() || HistoryNodeId::parse(head).is_err()
        })
    {
        return Err(invalid("deletion identity or complete frontier mismatch"));
    }
    let parents = node.parent_node_ids.iter().collect::<BTreeSet<_>>();
    if parents.len() != node.parent_node_ids.len()
        || parents != node.branch_heads.values().collect()
    {
        return Err(invalid(
            "deletion parent edges do not match all retained branch heads",
        ));
    }
    Ok(Some(node))
}
