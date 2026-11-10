use crate::api;
use data_serialization::{Deserialize, Deserializer};
use json_codec::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use storage_contract::{
    FileDeletionNode, OperationId,
    client_protocol::MAX_FILE_DELETION_FRONTIER_ENTRIES,
    directory_git::{DirectoryDeletionNode, DirectoryDeletionSummary, DirectoryDeletionsPage},
    history_window::{HistoryFileId, HistoryRepositoryId, HistoryTreeId},
};

const MAX_FILES: usize = 100;

fn error(source: json_codec::Error) -> api::Error {
    api::Error {
        status: 200,
        code: storage_contract::ErrorCode::InvalidResponse,
        message: "The directory deletion response did not match its request or contract.".into(),
        retryable: false,
        request_id: None,
        source: Some(Arc::new(api::ErrorSource::FileDeletion(source))),
    }
}
fn invalid(message: &str) -> api::Error {
    error(<json_codec::Error as data_serialization::de::Error>::custom(message))
}

pub(in crate::files::deletion) fn page(value: Value) -> Result<DirectoryDeletionsPage, api::Error> {
    if !value.is_object()
        || value["entries"].as_array().is_none_or(|entries| {
            entries.len() > MAX_FILES || entries.iter().any(|entry| !entry.is_object())
        })
    {
        return Err(invalid(
            "directory deletion page must contain bounded named entries",
        ));
    }
    let page = DirectoryDeletionsPage::deserialize(value).map_err(error)?;
    if page.snapshot.is_empty()
        || page.snapshot.len() > 256
        || page
            .entries
            .windows(2)
            .any(|pair| pair[0].directory_id >= pair[1].directory_id)
        || page.entries.iter().any(|entry| {
            storage_contract::validate_id(&entry.directory_id, "entry").is_err()
                || storage_contract::validate_id(&entry.deletion_node_id, "ddel").is_err()
                || !matches!(identity(entry.operation_id, "ddel", &entry.directory_id), Ok(id) if id == entry.deletion_node_id)
        })
        || page.next_after.as_ref().is_some_and(|after| {
            page.entries.len() != MAX_FILES
                || page
                    .entries
                    .last()
                    .is_none_or(|entry| &entry.directory_id != after)
        })
    {
        return Err(invalid(
            "invalid directory deletion page identity, order or cursor",
        ));
    }
    Ok(page)
}

pub(in crate::files::deletion) fn node(
    value: Value,
    repository: &str,
    owner: &str,
    summary: &DirectoryDeletionSummary,
) -> Result<Option<DirectoryDeletionNode>, api::Error> {
    if value.is_null() {
        return Ok(None);
    }
    let node = parse_node(value)?;
    if node.repository_id != repository
        || node.owner_directory_id != owner
        || node.directory_id != summary.directory_id
        || node.node_id != summary.deletion_node_id
        || node.path != summary.path
        || node.operation_id != summary.operation_id
    {
        return Err(invalid("directory deletion identity mismatch"));
    }
    Ok(Some(node))
}

pub(in crate::files::deletion) fn unchanged_node(
    value: Value,
    repository: &str,
    owner: &str,
    summary: &DirectoryDeletionSummary,
    expected: &DirectoryDeletionNode,
) -> Result<Option<DirectoryDeletionNode>, api::Error> {
    let current = node(value, repository, owner, summary)?;
    if current.as_ref().is_some_and(|current| current != expected) {
        return Err(invalid(
            "directory deletion changed while reviewing restoration",
        ));
    }
    Ok(current)
}

pub(in crate::files::deletion) fn parse_node(
    value: Value,
) -> Result<DirectoryDeletionNode, api::Error> {
    if !value.is_object()
        || value["files"].as_object().is_none_or(|files| {
            files.len() > MAX_FILES || files.values().any(|member| !member.is_object())
        })
    {
        return Err(invalid(
            "directory deletion must contain a complete named member map",
        ));
    }
    let node = DirectoryDeletionNode::deserialize(value).map_err(error)?;
    validate_node(&node)?;
    Ok(node)
}

pub(in crate::files::deletion) fn validate_node(
    node: &DirectoryDeletionNode,
) -> Result<(), api::Error> {
    if HistoryRepositoryId::parse(&node.repository_id).is_err()
        || HistoryTreeId::parse(&node.history_tree_id).is_err()
        || storage_contract::validate_id(&node.node_id, "ddel").is_err()
        || storage_contract::validate_id(&node.directory_id, "entry").is_err()
        || node.owner_directory_id != node.repository_id
            && storage_contract::validate_id(&node.owner_directory_id, "entry").is_err()
        || node.owner_directory_id == node.directory_id
        || storage_contract::validate_id(&node.namespace_head, "commit").is_err()
        || storage_contract::validate_id(&node.directory_branch_id, "dbr").is_err()
        || storage_contract::validate_id(&node.source_tree_id, "dtree").is_err()
        || node.sequence == 0
        || node.author.is_empty()
        || node.author.len() > 1024
        || node.files.len() > MAX_FILES
        || !matches!(identity(node.operation_id, "ddel", &node.directory_id), Ok(id) if id == node.node_id)
    {
        return Err(invalid(
            "invalid directory deletion identity or member bound",
        ));
    }
    let mut deletion_ids = BTreeSet::new();
    for (file, member) in &node.files {
        if HistoryFileId::parse(file).is_err()
            || storage_contract::validate_id(&member.deletion_node_id, "fdel").is_err()
            || !deletion_ids.insert(&member.deletion_node_id)
            || member.created_by_directory
                && !matches!(identity(node.operation_id, "fdel", file), Ok(id) if id == member.deletion_node_id)
        {
            return Err(invalid("invalid directory deletion member identity"));
        }
    }
    Ok(())
}

fn identity(operation: OperationId, prefix: &str, owner: &str) -> storage_contract::Result<String> {
    storage_contract::object_hash(
        "directory-deletion-identity",
        &json!({"operation_id":operation,"prefix":prefix,"owner":owner}),
    )
    .map(|hash| format!("{prefix}_{}", &hash[..32]))
}

pub(super) fn files_match(node: &DirectoryDeletionNode, value: &Value) -> bool {
    let Some(nodes) = value.as_array() else {
        return false;
    };
    let expected = node
        .files
        .iter()
        .filter(|(_, member)| member.created_by_directory);
    if nodes.len() != expected.clone().count() {
        return false;
    }
    let mut branches = 0;
    let mut parents = 0;
    for ((file, member), value) in expected.zip(nodes) {
        if !value.is_object() {
            return false;
        }
        let deletion = match FileDeletionNode::deserialize(value) {
            Ok(deletion) => deletion,
            Err(_) => return false,
        };
        if super::super::wire::validate_node(&deletion).is_err()
            || deletion.file_id != *file
            || deletion.node_id != member.deletion_node_id
            || deletion.repository_id != node.repository_id
            || deletion.history_tree_id != node.history_tree_id
            || deletion.operation_id != node.operation_id
            || deletion.author != node.author
            || deletion.sequence != node.sequence
            || deletion.created_at != node.created_at
            || !deletion
                .path
                .key()
                .strip_prefix(node.path.key())
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return false;
        }
        branches += deletion.branch_heads.len();
        parents += deletion.parent_node_ids.len();
        if branches > MAX_FILE_DELETION_FRONTIER_ENTRIES
            || parents > MAX_FILE_DELETION_FRONTIER_ENTRIES
        {
            return false;
        }
    }
    true
}

pub(in crate::files::deletion) fn undo_receipt_matches(
    node: &DirectoryDeletionNode,
    receipt: &Value,
) -> bool {
    receipt["operation_kind"] == "directory.delete.undo"
        && super::super::wire::publication_sequence(
            receipt,
            &[
                "restored_directory_id",
                "removed_directory_deletion",
                "removed_deletion_nodes",
            ],
        )
        .is_some_and(|sequence| sequence > node.sequence)
        && receipt["result"]["restored_directory_id"] == node.directory_id
        && receipt["result"]["removed_directory_deletion"] == json!(node)
        && files_match(node, &receipt["result"]["removed_deletion_nodes"])
}

pub(in crate::files::deletion) fn stored_node<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<DirectoryDeletionNode, D::Error> {
    let value = Value::deserialize(deserializer)?;
    parse_node(value).map_err(data_serialization::de::Error::custom)
}
