//! Semantics for fixed documentation inputs, not proof of runtime authorization,
//! history completeness, pre-encoding allocation bounds or current disk health.
use super::{Result, array, require, text, uint};
use json_codec::Value;
use std::collections::BTreeSet;
use storage_contract::history_window::{
    HistoryCollection, HistoryCursor, HistoryCursorPayload, HistoryDirection, HistoryPageIdentity,
    HistoryPageLimit, HistoryPosition, HistoryWindowPage, MAX_HISTORY_NODE_ENCODED_BYTES,
    MAX_HISTORY_PREVIEW_EDGES, MAX_HISTORY_RESPONSE_BYTES,
};

fn cursor_binding(cursor: &HistoryCursor, identity: &HistoryPageIdentity) -> Result {
    let payload = cursor.payload();
    require(
        payload.history_root == identity.history_root
            && payload.history_tree_id == identity.context.history_tree_id
            && payload.file_id == identity.file_id,
        "history cursor root or subject mismatch",
    )
}

fn page_cursor(
    value: &Value,
    identity: &HistoryPageIdentity,
    direction: HistoryDirection,
) -> Result {
    if value.is_null() {
        return Ok(());
    }
    let cursor = HistoryCursor::parse(text(value)?)?;
    cursor_binding(&cursor, identity)?;
    let payload = cursor.payload();
    require(
        payload.collection == identity.collection
            && payload.query_hash == identity.query_hash
            && payload.limit == identity.limit
            && payload.direction == direction,
        "history cursor query, collection, budget or direction mismatch",
    )
}

fn inline_fields(value: &Value) -> Result {
    if let Some(note) = value.get("note") {
        storage_contract::validate_note(text(note)?)?;
    }
    for field in [
        "path",
        "previous_path",
        "download_name",
        "storage_name",
        "name",
    ] {
        if let Some(name) = value.get(field).filter(|value| !value.is_null()) {
            let name = text(name)?;
            require(
                !name.is_empty() && name.len() <= 768 && name.encode_utf16().count() <= 200,
                "history inline name byte/unit budget",
            )?;
        }
    }
    if let Some(tags) = value.get("tags") {
        let tags = array(tags)?;
        require(
            tags.len() <= storage_contract::MAX_FILE_TAGS,
            "history tag count",
        )?;
        let mut seen = BTreeSet::new();
        for tag in tags {
            let tag = text(tag)?;
            storage_contract::validate_tag(tag)?;
            require(seen.insert(tag), "duplicate history tag")?;
        }
    }
    Ok(())
}

pub(super) fn validate(name: &str, value: &Value) -> Result {
    if name == "HistoryCursor" {
        HistoryCursor::parse(text(value)?)?;
        return Ok(());
    }
    let encoded = json_codec::to_vec(value)?;
    if name == "HistoryCursorPayload" {
        let payload: HistoryCursorPayload = storage_contract::decode_strict(&encoded)?;
        payload.validate()?;
        HistoryCursor::from_payload(payload)?;
        return Ok(());
    }
    // A fixture is already bounded source input. The production response writer
    // must prove its budget before encoding; this check makes no such claim.
    let _: HistoryWindowPage = storage_contract::decode_strict(&encoded)?;
    require(
        encoded.len() <= MAX_HISTORY_RESPONSE_BYTES,
        "history encoded page budget",
    )?;
    let identity: HistoryPageIdentity =
        storage_contract::decode_strict(&json_codec::to_vec(&value["identity"])?)?;
    HistoryPageLimit::for_collection(identity.limit.get(), &identity.collection)?;
    require(
        value["kind"] == value["identity"]["collection"]["kind"],
        "history page collection mismatch",
    )?;
    if value["kind"] == "node_field" {
        return field_fragment(value, &identity);
    }
    page_cursor(
        &value["page"]["previous_cursor"],
        &identity,
        HistoryDirection::Backward,
    )?;
    page_cursor(
        &value["page"]["next_cursor"],
        &identity,
        HistoryDirection::Forward,
    )?;
    let kind = text(&value["kind"])?;
    let member_key = match kind {
        "nodes" => "nodes",
        "versions" => "versions",
        "branches" => "branches",
        "merges" => "merges",
        "node_parents" | "merge_parents" => "parents",
        "version_sources" => "sources",
        "merge_inputs" => "inputs",
        _ => return Err("unsupported history page kind".into()),
    };
    let members = array(&value[member_key])?;
    require(
        members.len() <= identity.limit.get(),
        "history record page budget",
    )?;
    let mut previous_ordinal = None;
    let mut previous_parent = None;
    for member in members {
        inline_fields(member)?;
        if let Some(ordinal) = member.get("ordinal") {
            let ordinal = uint(ordinal)?;
            require(
                previous_ordinal.is_none_or(|last| last < ordinal),
                "history member ordinal order",
            )?;
            previous_ordinal = Some(ordinal);
        }
        if kind == "node_parents" {
            let parent = text(&member["parent_node_id"])?;
            require(
                previous_parent.is_none_or(|last| last < parent),
                "history parent ID order",
            )?;
            previous_parent = Some(parent);
        }
    }
    if kind == "nodes" {
        inline_fields(&value["context"])?;
        nodes(value, &identity)?;
    }
    Ok(())
}

fn nodes(value: &Value, identity: &HistoryPageIdentity) -> Result {
    let nodes = array(&value["nodes"])?;
    let mut ids = BTreeSet::new();
    for node in nodes {
        let id = text(&node["node_id"])?;
        require(ids.insert(id), "duplicate history node")?;
        require(
            json_codec::to_vec(node)?.len() <= MAX_HISTORY_NODE_ENCODED_BYTES,
            "history node encoded budget",
        )?;
        if node["kind"] == "content" {
            require(
                node["node_id"] == node["version_id"],
                "content node is not its version",
            )?;
        }
        if node["parents"]["state"] == "more" {
            let cursor = HistoryCursor::parse(text(&node["parents"]["cursor"])?)?;
            cursor_binding(&cursor, identity)?;
            require(
                matches!(&cursor.payload().collection,
                    HistoryCollection::NodeParents { node_id } if node_id.as_str() == id)
                    && cursor.payload().direction == HistoryDirection::Forward,
                "parent continuation subject or direction mismatch",
            )?;
        }
    }
    let edges = array(&value["edges"])?;
    require(
        edges.len() <= MAX_HISTORY_PREVIEW_EDGES,
        "history edge preview budget",
    )?;
    let mut seen = BTreeSet::new();
    for edge in edges {
        let child = text(&edge["node_id"])?;
        let parent = text(&edge["parent_node_id"])?;
        require(
            ids.contains(child) && seen.insert((child, parent)),
            "invalid preview edge child/pair",
        )?;
        require(
            (edge["endpoint"] == "in_window") == ids.contains(parent),
            "history edge window endpoint mismatch",
        )?;
    }
    Ok(())
}

fn field_fragment(value: &Value, identity: &HistoryPageIdentity) -> Result {
    require(
        value["field"] == value["identity"]["collection"]["field"],
        "history field selector mismatch",
    )?;
    let total = uint(&value["total_bytes"])?;
    let offset = uint(&value["byte_offset"])?;
    let fragment = text(&value["text"])?;
    require(
        fragment.len() <= identity.limit.get(),
        "history field UTF-8 byte budget",
    )?;
    let end = offset
        .checked_add(fragment.len() as u64)
        .ok_or("history field offset overflow")?;
    require(end <= total, "history field exceeds total")?;
    require(
        value["end_of_field"] == (end == total),
        "history field right boundary",
    )?;
    require(
        total == 0 || !fragment.is_empty(),
        "history field must advance",
    )?;
    if end == total {
        require(
            value["next_cursor"].is_null(),
            "history field end has continuation",
        )?;
    } else {
        require(
            !value["next_cursor"].is_null(),
            "history field missing continuation",
        )?;
        page_cursor(&value["next_cursor"], identity, HistoryDirection::Forward)?;
        let cursor = HistoryCursor::parse(text(&value["next_cursor"])?)?;
        require(
            matches!(&cursor.payload().position, HistoryPosition::Field { byte_offset } if *byte_offset == end),
            "history field continuation offset",
        )?;
    }
    Ok(())
}
