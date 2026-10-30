//! Encode one already selected history page once, with a hard limit before
//! each buffer extension. The HTTP body owner retains the existing admission.
use crate::ApiError;
use data_serialization::Serialize;
use std::io::{self, Write};
use storage_contract::{ErrorCode, history_window::*};

fn invalid(message: &str) -> ApiError {
    ApiError::new(
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::StateInvalid,
        message,
    )
}

fn bounded_error(source: json_codec::Error) -> ApiError {
    ApiError::new(
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::ResponseTooLarge,
        "history page exceeds its encoded byte budget",
    )
    .with_source(source)
}

struct PageBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl PageBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

fn admit_write(used: usize, limit: usize, count: usize) -> io::Result<()> {
    if count > limit - used {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history JSON byte budget exhausted",
        ));
    }
    Ok(())
}

impl Write for PageBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        admit_write(self.bytes.len(), self.limit, bytes.len())?;
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().max(256) * 2)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CountBytes {
    used: usize,
    limit: usize,
}
impl Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        admit_write(self.used, self.limit, bytes.len())?;
        self.used += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bounded_record(value: &impl Serialize, limit: usize) -> Result<(), ApiError> {
    json_codec::to_writer(&mut CountBytes { used: 0, limit }, value).map_err(bounded_error)
}

fn count(identity: &HistoryPageIdentity, rows: usize) -> Result<(), ApiError> {
    if identity.limit.get() > identity.collection.maximum_limit() || rows > identity.limit.get() {
        return Err(invalid("history collection exceeds its record budget"));
    }
    Ok(())
}

fn scoped_cursor(cursor: &HistoryCursor, identity: &HistoryPageIdentity) -> Result<(), ApiError> {
    let payload = cursor.payload();
    if payload.history_root != identity.history_root
        || payload.file_id != identity.file_id
        || payload.history_tree_id != identity.context.history_tree_id
    {
        return Err(invalid("history page cursor belongs to another scope"));
    }
    Ok(())
}

fn page_cursor(
    cursor: &HistoryCursor,
    identity: &HistoryPageIdentity,
    direction: HistoryDirection,
) -> Result<(), ApiError> {
    scoped_cursor(cursor, identity)?;
    let payload = cursor.payload();
    if payload.collection != identity.collection
        || payload.query_hash != identity.query_hash
        || payload.limit != identity.limit
        || payload.direction != direction
    {
        return Err(invalid("history page cursor does not match its query"));
    }
    Ok(())
}

fn cursors(page: &HistoryPageCursors, identity: &HistoryPageIdentity) -> Result<(), ApiError> {
    if let Some(cursor) = &page.previous_cursor {
        page_cursor(cursor, identity, HistoryDirection::Backward)?;
    }
    if let Some(cursor) = &page.next_cursor {
        page_cursor(cursor, identity, HistoryDirection::Forward)?;
    }
    if page.previous_cursor.is_some() && page.previous_cursor == page.next_cursor {
        return Err(invalid(
            "history page directions share a non-progressing cursor",
        ));
    }
    Ok(())
}

fn note(value: &str) -> Result<(), ApiError> {
    storage_contract::validate_note(value)
        .map_err(|source| invalid("history summary has an invalid note").with_source(source))
}

fn name(value: &str) -> Result<(), ApiError> {
    name_policy::validate_name(value)
        .map_err(|source| invalid("history summary has an invalid name").with_source(source))
}

fn node(node: &HistoryNodeSummary, identity: &HistoryPageIdentity) -> Result<(), ApiError> {
    name_policy::validate_path(&node.path)
        .map_err(|source| invalid("history node has an invalid path").with_source(source))?;
    if let Some(path) = &node.previous_path {
        name_policy::validate_path(path).map_err(|source| {
            invalid("history node has an invalid previous path").with_source(source)
        })?;
    }
    name(&node.download_name)?;
    name(&node.storage_name)?;
    note(&node.note)?;
    if node.tags.len() > storage_contract::MAX_FILE_TAGS {
        return Err(invalid("history node exceeds its tag budget"));
    }
    for tag in &node.tags {
        storage_contract::validate_tag(tag)
            .map_err(|source| invalid("history node has an invalid tag").with_source(source))?;
    }
    if let HistoryParentSet::More { cursor } = &node.parents {
        scoped_cursor(cursor, identity)?;
        if !matches!(&cursor.payload().collection,
            HistoryCollection::NodeParents { node_id } if node_id == &node.node_id)
            || cursor.payload().direction != HistoryDirection::Forward
        {
            return Err(invalid(
                "history parent continuation belongs to another node",
            ));
        }
    }
    bounded_record(node, MAX_HISTORY_NODE_ENCODED_BYTES)
}

struct FieldFragment<'a> {
    field: HistoryField,
    total: u64,
    offset: u64,
    text: &'a str,
    end_of_field: bool,
    next: Option<&'a HistoryCursor>,
}

fn fragment(identity: &HistoryPageIdentity, fragment: FieldFragment<'_>) -> Result<(), ApiError> {
    let FieldFragment {
        field,
        total,
        offset,
        text,
        end_of_field,
        next,
    } = fragment;
    if !matches!(&identity.collection, HistoryCollection::NodeField { field: selected, .. } if *selected == field)
    {
        return Err(invalid(
            "history field response does not match its collection",
        ));
    }
    count(identity, 0)?;
    if text.len() > identity.limit.get() || text.len() > MAX_HISTORY_FIELD_BYTES {
        return Err(invalid("history field exceeds its fragment budget"));
    }
    let end = offset
        .checked_add(text.len() as u64)
        .ok_or_else(|| invalid("history field byte range overflows"))?;
    if end > total
        || end_of_field != (end == total)
        || (total > 0 && text.is_empty())
        || (total == 0 && offset != 0)
        || end_of_field == next.is_some()
    {
        return Err(invalid(
            "history field fragment does not describe a progressing range",
        ));
    }
    if let Some(cursor) = next {
        page_cursor(cursor, identity, HistoryDirection::Forward)?;
        if cursor.payload().position != (HistoryPosition::Field { byte_offset: end }) {
            return Err(invalid(
                "history field continuation has an incorrect byte offset",
            ));
        }
    }
    Ok(())
}

fn validate(page: &HistoryWindowPage) -> Result<(), ApiError> {
    match page {
        HistoryWindowPage::Nodes {
            identity,
            context,
            nodes,
            edges,
            page,
        } => {
            if identity.collection != HistoryCollection::Nodes {
                return Err(invalid("history nodes response has a different collection"));
            }
            count(identity, nodes.len())?;
            if edges.len() > MAX_HISTORY_PREVIEW_EDGES {
                return Err(invalid("history parent preview exceeds its edge budget"));
            }
            note(&context.note)?;
            cursors(page, identity)?;
            for value in nodes {
                node(value, identity)?;
            }
            for edge in edges {
                if !nodes.iter().any(|node| node.node_id == edge.node_id) {
                    return Err(invalid("history preview child is not in its window"));
                }
                let in_window = nodes.iter().any(|node| node.node_id == edge.parent_node_id);
                if in_window != (edge.endpoint == HistoryEndpoint::InWindow) {
                    return Err(invalid(
                        "history preview endpoint has an incorrect window state",
                    ));
                }
            }
        }
        HistoryWindowPage::Versions {
            identity,
            versions,
            page,
        } => {
            if identity.collection != HistoryCollection::Versions {
                return Err(invalid(
                    "history versions response has a different collection",
                ));
            }
            count(identity, versions.len())?;
            cursors(page, identity)?;
            for value in versions {
                note(&value.note)?;
                name(&value.download_name)?;
                name(&value.storage_name)?;
            }
        }
        HistoryWindowPage::Branches {
            identity,
            branches,
            page,
        } => {
            if identity.collection != HistoryCollection::Branches {
                return Err(invalid(
                    "history branches response has a different collection",
                ));
            }
            count(identity, branches.len())?;
            cursors(page, identity)?;
            for value in branches {
                storage_contract::validate_branch_name(&value.name).map_err(|source| {
                    invalid("history branch has an invalid name").with_source(source)
                })?;
            }
        }
        HistoryWindowPage::Merges {
            identity,
            merges,
            page,
        } => {
            if identity.collection != HistoryCollection::Merges {
                return Err(invalid(
                    "history merges response has a different collection",
                ));
            }
            count(identity, merges.len())?;
            cursors(page, identity)?;
        }
        HistoryWindowPage::NodeParents {
            identity,
            parents,
            page,
        } => {
            if !matches!(identity.collection, HistoryCollection::NodeParents { .. }) {
                return Err(invalid(
                    "history parents response has a different collection",
                ));
            }
            count(identity, parents.len())?;
            cursors(page, identity)?;
        }
        HistoryWindowPage::VersionSources {
            identity,
            sources,
            page,
        } => {
            if !matches!(
                identity.collection,
                HistoryCollection::VersionSources { .. }
            ) {
                return Err(invalid(
                    "history sources response has a different collection",
                ));
            }
            count(identity, sources.len())?;
            cursors(page, identity)?;
        }
        HistoryWindowPage::MergeInputs {
            identity,
            inputs,
            page,
        } => {
            if !matches!(identity.collection, HistoryCollection::MergeInputs { .. }) {
                return Err(invalid(
                    "history merge inputs response has a different collection",
                ));
            }
            count(identity, inputs.len())?;
            cursors(page, identity)?;
        }
        HistoryWindowPage::MergeParents {
            identity,
            parents,
            page,
        } => {
            if !matches!(identity.collection, HistoryCollection::MergeParents { .. }) {
                return Err(invalid(
                    "history merge parents response has a different collection",
                ));
            }
            count(identity, parents.len())?;
            cursors(page, identity)?;
        }
        HistoryWindowPage::NodeField {
            identity,
            field,
            total_bytes,
            byte_offset,
            text,
            end_of_field,
            next_cursor,
        } => {
            fragment(
                identity,
                FieldFragment {
                    field: *field,
                    total: *total_bytes,
                    offset: *byte_offset,
                    text,
                    end_of_field: *end_of_field,
                    next: next_cursor.as_ref(),
                },
            )?;
        }
    }
    Ok(())
}

pub(crate) fn encode(page: &HistoryWindowPage) -> Result<Vec<u8>, ApiError> {
    validate(page)?;
    let mut buffer = PageBuffer::new(MAX_HISTORY_RESPONSE_BYTES);
    json_codec::to_writer(&mut buffer, page).map_err(bounded_error)?;
    Ok(buffer.bytes)
}

#[cfg(test)]
#[path = "../../../test/server/unit/history_window_encode.rs"]
mod tests;
