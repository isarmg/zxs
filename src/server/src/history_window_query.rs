//! Static history-window query decoding. Cursor contents are positions, not
//! admission: current identity, ownership and query/root matching remain live
//! checks in the backend after this boundary has parsed the external values.
use crate::ApiError;
use std::collections::BTreeMap;
use storage_contract::history_window::{
    HistoryCollection, HistoryCursor, HistoryField, HistoryFileId, HistoryLocation, HistoryNodeId,
    HistoryPageLimit, HistoryRepositoryId, HistoryRoot, HistoryWindowRequest,
};

/// `ApiQuery` rejects duplicate decoded keys before producing this map.
pub(crate) fn parse(
    file_id: &str,
    collection: HistoryCollection,
    mut query: BTreeMap<String, String>,
) -> Result<HistoryWindowRequest, ApiError> {
    if let Some(key) = query.keys().find(|key| {
        !matches!(
            key.as_str(),
            "history_root" | "cursor" | "limit" | "repository_id" | "path" | "anchor_node_id"
        )
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let file_id = HistoryFileId::parse(file_id).map_err(ApiError::contract_input)?;
    let location = match (query.remove("repository_id"), query.remove("path")) {
        (None, None) => None,
        (Some(repository_id), Some(path)) => Some(HistoryLocation::new(
            HistoryRepositoryId::parse(&repository_id).map_err(ApiError::contract_input)?,
            name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source)?,
        )),
        (Some(_), None) | (None, Some(_)) => {
            return Err(ApiError::invalid(
                "repository_id and path must be supplied together",
            ));
        }
    };
    let history_root = query
        .remove("history_root")
        .map(|value| HistoryRoot::parse(&value).map_err(ApiError::contract_input))
        .transpose()?;
    let cursor = query
        .remove("cursor")
        .map(|value| HistoryCursor::parse(&value).map_err(ApiError::invalid_source))
        .transpose()?;
    let anchor_node_id = query
        .remove("anchor_node_id")
        .map(|value| HistoryNodeId::parse(&value).map_err(ApiError::contract_input))
        .transpose()?;
    if anchor_node_id.is_some() {
        if collection != HistoryCollection::Nodes {
            return Err(ApiError::invalid(
                "anchor_node_id is only valid for the node window",
            ));
        }
        if cursor.is_some() {
            return Err(ApiError::invalid(
                "anchor_node_id and cursor are mutually exclusive",
            ));
        }
        if history_root.is_none() {
            return Err(ApiError::invalid("anchor_node_id requires history_root"));
        }
    }
    if collection.requires_root() && history_root.is_none() && cursor.is_none() {
        return Err(ApiError::invalid(
            "history_root or cursor is required for this history collection",
        ));
    }
    let limit = match query.remove("limit") {
        Some(value) => {
            let value =
                storage_contract::parse_u64_decimal(&value).map_err(ApiError::invalid_source)?;
            let value = usize::try_from(value).map_err(ApiError::invalid_source)?;
            HistoryPageLimit::for_collection(value, &collection)
                .map_err(ApiError::contract_input)?
        }
        None => cursor.as_ref().map_or_else(
            || HistoryPageLimit::default_for(&collection),
            |cursor| cursor.payload().limit,
        ),
    };
    Ok(HistoryWindowRequest {
        file_id,
        collection,
        location,
        history_root,
        cursor,
        anchor_node_id,
        limit,
    })
}

/// A closed scalar selector, never a JSON path or arbitrary stored field name.
pub(crate) fn field(value: &str) -> Result<HistoryField, ApiError> {
    match value {
        "message" => Ok(HistoryField::Message),
        "author" => Ok(HistoryField::Author),
        "created_at" => Ok(HistoryField::CreatedAt),
        _ => Err(ApiError::invalid("unknown history field")),
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/history_window_query.rs"]
mod tests;
