//! Network admission; renderers only consume these accepted bounded pages.
use super::*;
use data_serialization::de::DeserializeOwned;
use storage_contract::{
    ErrorCode,
    client_protocol::{
        MAX_HISTORY_PAGE_BYTES, MAX_INLINE_TEXT_BYTES, ResponseDecodeError, ResponseError,
    },
};

fn invalid(message: &'static str) -> api::Error {
    ResponseError::new(200, ErrorCode::InvalidResponse, message).into()
}
fn require(condition: bool, message: &'static str) -> Result<(), api::Error> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}
fn decode<T: DeserializeOwned>(status: u16, bytes: &[u8]) -> Result<T, api::Error> {
    if !(200..300).contains(&status) {
        return Err(
            storage_contract::client_protocol::decode_response(status, bytes)
                .expect_err("invariant: non-success status decodes as an API error")
                .into(),
        );
    }
    require(
        bytes.len() <= MAX_HISTORY_PAGE_BYTES,
        "History graph response exceeds its page budget",
    )?;
    json_codec::from_slice(bytes).map_err(|source| {
        let mut error = ResponseError::new(
            status,
            ErrorCode::InvalidResponse,
            "Invalid history graph response",
        );
        error.source = Some(ResponseDecodeError::Json(source));
        error.into()
    })
}
fn root(value: &str) -> Result<(), api::Error> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid history graph root",
    )
}
fn identity(value: &str) -> Result<(), api::Error> {
    require(
        !value.is_empty() && value.len() <= 256,
        "Invalid history graph identity",
    )
}
fn text(value: &TextSummary) -> Result<(), api::Error> {
    match value {
        TextSummary::Inline { text } => require(
            text.len() <= MAX_INLINE_TEXT_BYTES,
            "Oversized inline graph text",
        ),
        TextSummary::Deferred { bytes } => require(
            *bytes > MAX_INLINE_TEXT_BYTES as u64,
            "Invalid deferred graph text size",
        ),
    }
}
fn optional_text(value: &Option<TextSummary>) -> Result<(), api::Error> {
    if let Some(value) = value {
        text(value)?;
    }
    Ok(())
}
fn rows<T>(
    records: &[T],
    history: &str,
    id: impl Fn(&T) -> &str,
    owner: impl Fn(&T) -> &str,
) -> Result<(), api::Error> {
    require(records.len() <= 50, "Too many graph records")?;
    for record in records {
        identity(id(record))?;
        require(
            owner(record) == history,
            "Graph record belongs to another history",
        )?;
    }
    Ok(())
}
fn ordered_ids<T>(records: &[T], id: impl Fn(&T) -> &str) -> Result<(), api::Error> {
    require(
        records.windows(2).all(|pair| id(&pair[0]) < id(&pair[1])),
        "Graph identities are duplicated or out of order",
    )
}
pub(super) fn main_page(
    status: u16,
    bytes: &[u8],
    history: &str,
    collection: GraphCollection,
    expected_root: Option<&str>,
) -> Result<GraphPage, api::Error> {
    let page: GraphPage = decode(status, bytes)?;
    require(
        page.history.history_tree_id == history && page.items.collection() == collection,
        "Graph response differs from requested history or collection",
    )?;
    root(&page.history_root)?;
    if let Some(expected) = expected_root {
        require(
            page.history_root == expected,
            "Graph response differs from requested root",
        )?;
    }
    identity(&page.history.repository_id)?;
    identity(&page.history.root_commit_id)?;
    if let Some(id) = &page.history.main_branch_id {
        identity(id)?;
    }
    optional_text(&page.history.provenance)?;
    let count = match &page.items {
        GraphItems::Commits(records) => {
            rows(records, history, |r| &r.commit_id, |r| &r.history_tree_id)?;
            let mut identities = std::collections::BTreeSet::new();
            require(
                records.windows(2).all(|pair| {
                    (pair[0].sequence, &pair[0].commit_id) < (pair[1].sequence, &pair[1].commit_id)
                }),
                "Graph commits are duplicated or out of order",
            )?;
            for record in records {
                require(
                    identities.insert(&record.commit_id),
                    "Duplicate graph commit identity",
                )?;
                require(
                    record.parent_commit_ids.len()
                        == usize::from(record.commit_id != page.history.root_commit_id),
                    "Invalid graph commit parent count",
                )?;
                for id in &record.parent_commit_ids {
                    identity(id)?;
                }
                for value in [
                    &record.kind,
                    &record.tree_id,
                    &record.author,
                    &record.message,
                ] {
                    text(value)?;
                }
                optional_text(&record.provenance)?;
                root(&record.namespace.root.hash)?;
                if let Some(deletions) = &record.namespace_deletions {
                    require(deletions.paths > 0, "Empty graph deletion facts")?;
                }
            }
            records.len()
        }
        GraphItems::Branches(records) => {
            rows(records, history, |r| &r.branch_id, |r| &r.history_tree_id)?;
            ordered_ids(records, |r| &r.branch_id)?;
            for record in records {
                require(record.name.len() <= 128, "Oversized graph branch name")?;
                identity(&record.head_commit_id)?;
                identity(&record.fork_commit_id)?;
                if let Some(id) = &record.file_id {
                    identity(id)?;
                }
                if let Some(id) = &record.file_node_id {
                    identity(id)?;
                }
            }
            records.len()
        }
        GraphItems::MergeLinks(records) => {
            rows(
                records,
                history,
                |r| &r.merge_link_id,
                |r| &r.history_tree_id,
            )?;
            ordered_ids(records, |r| &r.merge_link_id)?;
            for record in records {
                for id in [
                    &record.source_branch_id,
                    &record.source_commit_id,
                    &record.target_branch_id,
                    &record.target_commit_id,
                ] {
                    identity(id)?;
                }
                text(&record.author)?;
            }
            records.len()
        }
        GraphItems::BoundaryMarks(records) => {
            rows(records, history, |r| &r.boundary_id, |r| &r.history_tree_id)?;
            ordered_ids(records, |r| &r.boundary_id)?;
            for record in records {
                for id in [
                    &record.local_commit_id,
                    &record.external_history_tree_id,
                    &record.external_commit_id,
                ] {
                    identity(id)?;
                }
                if let Some(id) = &record.local_branch_id {
                    identity(id)?;
                }
                text(&record.relation)?;
                optional_text(&record.source_merge_link_id)?;
            }
            records.len()
        }
    };
    if let Some(cursor) = &page.next_cursor {
        require(
            count > 0
                && !cursor.is_empty()
                && cursor.len() <= 32768
                && cursor.bytes().all(|b| b.is_ascii_graphic()),
            "Invalid graph continuation",
        )?;
    }
    Ok(page)
}
fn binding(
    history: &str,
    root: &str,
    actual_history: &str,
    actual_root: &str,
) -> Result<(), api::Error> {
    require(
        history == actual_history && root == actual_root,
        "Graph detail differs from the selected history root",
    )
}
pub(super) fn detail(
    status: u16,
    bytes: &[u8],
    history: &str,
    root: &str,
    request: &DetailRequest,
) -> Result<Detail, api::Error> {
    match request {
        DetailRequest::Text { target, offset } => {
            let page: TextPage = decode(status, bytes)?;
            binding(history, root, &page.history_tree_id, &page.history_root)?;
            require(
                &page.target == target && page.offset == *offset,
                "Graph text differs from requested field or offset",
            )?;
            match &page.text {
                Some(text) => {
                    require(
                        text.len() <= MAX_GRAPH_TEXT_BYTES,
                        "Oversized graph text slice",
                    )?;
                    let end = page
                        .offset
                        .checked_add(text.len() as u64)
                        .ok_or_else(|| invalid("Graph text offset overflow"))?;
                    require(
                        end <= page.total_bytes
                            && page.next_offset == (end < page.total_bytes).then_some(end),
                        "Invalid graph text continuation",
                    )?;
                    require(
                        end == page.total_bytes || !text.is_empty(),
                        "Empty continuing graph text slice",
                    )?;
                }
                None => require(
                    page.offset == 0 && page.total_bytes == 0 && page.next_offset.is_none(),
                    "Invalid absent graph text",
                )?,
            }
            Ok(Detail::Text(page))
        }
        DetailRequest::Pending { target, after } => {
            let page: PendingPage = decode(status, bytes)?;
            binding(history, root, &page.history_tree_id, &page.history_root)?;
            require(
                &page.target == target && page.records.len() <= MAX_GRAPH_RECORDS,
                "Invalid graph pending page",
            )?;
            ordered_ids(&page.records, |r| &r.file_id)?;
            for record in &page.records {
                identity(&record.file_id)?;
                identity(&record.version_id)?;
                if let Some(after) = after {
                    require(
                        record.file_id > *after,
                        "Graph pending page precedes its continuation",
                    )?;
                }
            }
            if let Some(next) = &page.next_after {
                require(
                    page.records
                        .last()
                        .is_some_and(|record| &record.file_id == next),
                    "Invalid graph pending continuation",
                )?;
            }
            Ok(Detail::Pending(page))
        }
        DetailRequest::Deletions { commit, offset } => {
            let page: DeletionsPage = decode(status, bytes)?;
            binding(history, root, &page.history_tree_id, &page.history_root)?;
            require(
                &page.commit_id == commit
                    && page.offset == *offset
                    && page.records.len() <= MAX_GRAPH_RECORDS,
                "Invalid graph deletion page",
            )?;
            let end = page
                .offset
                .checked_add(page.records.len() as u64)
                .ok_or_else(|| invalid("Graph deletion offset overflow"))?;
            require(
                end <= page.total && page.next_offset == (end < page.total).then_some(end),
                "Invalid graph deletion continuation",
            )?;
            require(
                page.timestamp.is_some() == (page.total > 0),
                "Invalid graph deletion presence",
            )?;
            require(
                end == page.total || !page.records.is_empty(),
                "Empty continuing graph deletion page",
            )?;
            for record in &page.records {
                require(record.path.len() <= 768, "Oversized deleted path")?;
            }
            Ok(Detail::Deletions(page))
        }
    }
}
