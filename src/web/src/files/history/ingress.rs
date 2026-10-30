//! A network page is admitted against the exact request before entering UI state.
//! The API reader already enforces the shared response-byte limit.
use super::DecodeError;
use json_codec::Value;
use std::collections::BTreeSet;
use storage_contract::history_window as wire;

pub(super) fn admit(
    value: Value,
    request: &wire::HistoryWindowRequest,
    expected_context: Option<&wire::HistoryResolvedContext>,
) -> Result<wire::HistoryWindowPage, DecodeError> {
    nullable_fields(&value)?;
    let page: wire::HistoryWindowPage = json_codec::from_value(value).map_err(DecodeError::Wire)?;
    let identity = identity(&page);
    if !collection_matches(&page, &identity.collection) {
        return Err(DecodeError::Relation(
            "history page variant differs from its collection",
        ));
    }
    admit_identity(identity, request, expected_context)?;
    match &page {
        wire::HistoryWindowPage::Nodes {
            context,
            nodes,
            edges,
            page,
            ..
        } => {
            storage_contract::validate_note(&context.note).map_err(DecodeError::Name)?;
            if let Some(content) = &context.content {
                content_disposition(content)?;
            }
            nodes_page(nodes, edges, identity, request)?;
            cursors(page, nodes.len(), identity, request)?;
        }
        wire::HistoryWindowPage::Versions { versions, page, .. } => {
            versions_page(versions, request.limit.get())?;
            cursors(page, versions.len(), identity, request)?;
        }
        wire::HistoryWindowPage::Branches { branches, page, .. } => {
            branches_page(branches, identity, request.limit.get())?;
            cursors(page, branches.len(), identity, request)?;
        }
        wire::HistoryWindowPage::Merges { merges, page, .. } => {
            merges_page(merges, request.limit.get())?;
            cursors(page, merges.len(), identity, request)?;
        }
        wire::HistoryWindowPage::NodeParents { parents, page, .. } => {
            bounded(parents.len(), request.limit.get())?;
            ordered(parents.iter().map(|parent| &parent.parent_node_id))?;
            cursors(page, parents.len(), identity, request)?;
        }
        wire::HistoryWindowPage::VersionSources { sources, page, .. } => {
            bounded(sources.len(), request.limit.get())?;
            ordinals(sources.iter().map(|source| source.ordinal))?;
            cursors(page, sources.len(), identity, request)?;
        }
        wire::HistoryWindowPage::MergeInputs { inputs, page, .. } => {
            bounded(inputs.len(), request.limit.get())?;
            ordinals(inputs.iter().map(|input| input.ordinal))?;
            cursors(page, inputs.len(), identity, request)?;
        }
        wire::HistoryWindowPage::MergeParents { parents, page, .. } => {
            bounded(parents.len(), request.limit.get())?;
            ordinals(parents.iter().map(|parent| parent.ordinal))?;
            cursors(page, parents.len(), identity, request)?;
        }
        wire::HistoryWindowPage::NodeField {
            field,
            total_bytes,
            byte_offset,
            text,
            end_of_field,
            next_cursor,
            ..
        } => fragment(
            FieldFragment {
                field: *field,
                total: *total_bytes,
                offset: *byte_offset,
                text,
                end_of_field: *end_of_field,
            },
            next_cursor.as_ref(),
            identity,
            request,
        )?,
    }
    Ok(page)
}

fn identity(page: &wire::HistoryWindowPage) -> &wire::HistoryPageIdentity {
    match page {
        wire::HistoryWindowPage::Nodes { identity, .. }
        | wire::HistoryWindowPage::Versions { identity, .. }
        | wire::HistoryWindowPage::Branches { identity, .. }
        | wire::HistoryWindowPage::Merges { identity, .. }
        | wire::HistoryWindowPage::NodeParents { identity, .. }
        | wire::HistoryWindowPage::VersionSources { identity, .. }
        | wire::HistoryWindowPage::MergeInputs { identity, .. }
        | wire::HistoryWindowPage::MergeParents { identity, .. }
        | wire::HistoryWindowPage::NodeField { identity, .. } => identity,
    }
}

fn collection_matches(
    page: &wire::HistoryWindowPage,
    collection: &wire::HistoryCollection,
) -> bool {
    matches!(
        (page, collection),
        (
            wire::HistoryWindowPage::Nodes { .. },
            wire::HistoryCollection::Nodes
        ) | (
            wire::HistoryWindowPage::Versions { .. },
            wire::HistoryCollection::Versions
        ) | (
            wire::HistoryWindowPage::Branches { .. },
            wire::HistoryCollection::Branches
        ) | (
            wire::HistoryWindowPage::Merges { .. },
            wire::HistoryCollection::Merges
        ) | (
            wire::HistoryWindowPage::NodeParents { .. },
            wire::HistoryCollection::NodeParents { .. }
        ) | (
            wire::HistoryWindowPage::VersionSources { .. },
            wire::HistoryCollection::VersionSources { .. }
        ) | (
            wire::HistoryWindowPage::MergeInputs { .. },
            wire::HistoryCollection::MergeInputs { .. }
        ) | (
            wire::HistoryWindowPage::MergeParents { .. },
            wire::HistoryCollection::MergeParents { .. }
        ) | (
            wire::HistoryWindowPage::NodeField { .. },
            wire::HistoryCollection::NodeField { .. }
        )
    )
}

fn admit_identity(
    identity: &wire::HistoryPageIdentity,
    request: &wire::HistoryWindowRequest,
    expected_context: Option<&wire::HistoryResolvedContext>,
) -> Result<(), DecodeError> {
    wire::HistoryPageLimit::for_collection(request.limit.get(), &request.collection)
        .map_err(DecodeError::Name)?;
    if identity.file_id != request.file_id
        || identity.collection != request.collection
        || identity.limit != request.limit
        || identity.action_max_entries == 0
        || request
            .history_root
            .is_some_and(|root| root != identity.history_root)
        || expected_context.is_some_and(|context| context != &identity.context)
    {
        return Err(DecodeError::Relation(
            "history page differs from its request",
        ));
    }
    if let Some(location) = &request.location
        && (identity.context.repository_id != location.repository_id
            || identity
                .context
                .path
                .as_ref()
                .map(name_policy::PortablePath::key)
                != Some(location.path.key()))
    {
        return Err(DecodeError::Relation(
            "history page changed its requested location",
        ));
    }
    let anchor = request.anchor_node_id.as_ref().or_else(|| {
        request
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.payload().anchor_node_id.as_ref())
    });
    if identity.query_hash != request.query_hash(&identity.context, anchor) {
        return Err(DecodeError::Relation("history page query identity differs"));
    }
    if let Some(cursor) = &request.cursor {
        cursor_identity(cursor, identity, anchor)?;
    } else if request.collection.requires_root() && request.history_root.is_none() {
        return Err(DecodeError::Relation(
            "auxiliary history reads require a root",
        ));
    }
    Ok(())
}

fn cursor_identity(
    cursor: &wire::HistoryCursor,
    identity: &wire::HistoryPageIdentity,
    anchor: Option<&wire::HistoryNodeId>,
) -> Result<(), DecodeError> {
    let payload = cursor.payload();
    if payload.history_root != identity.history_root
        || payload.history_tree_id != identity.context.history_tree_id
        || payload.file_id != identity.file_id
        || payload.collection != identity.collection
        || payload.query_hash != identity.query_hash
        || payload.limit != identity.limit
        || payload.anchor_node_id.as_ref() != anchor
    {
        return Err(DecodeError::Relation(
            "history cursor differs from its page",
        ));
    }
    Ok(())
}

fn cursors(
    page: &wire::HistoryPageCursors,
    count: usize,
    identity: &wire::HistoryPageIdentity,
    request: &wire::HistoryWindowRequest,
) -> Result<(), DecodeError> {
    if count == 0 && (page.previous_cursor.is_some() || page.next_cursor.is_some()) {
        return Err(DecodeError::Relation("empty history pages cannot continue"));
    }
    for (cursor, direction) in [
        (
            page.previous_cursor.as_ref(),
            wire::HistoryDirection::Backward,
        ),
        (page.next_cursor.as_ref(), wire::HistoryDirection::Forward),
    ] {
        if let Some(cursor) = cursor {
            response_cursor(cursor, direction, identity, request)?;
        }
    }
    Ok(())
}

fn response_cursor(
    cursor: &wire::HistoryCursor,
    direction: wire::HistoryDirection,
    identity: &wire::HistoryPageIdentity,
    request: &wire::HistoryWindowRequest,
) -> Result<(), DecodeError> {
    let anchor = request.anchor_node_id.as_ref().or_else(|| {
        request
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.payload().anchor_node_id.as_ref())
    });
    cursor_identity(cursor, identity, anchor)?;
    if cursor.payload().direction != direction
        || request
            .cursor
            .as_ref()
            .is_some_and(|previous| previous.payload() == cursor.payload())
    {
        return Err(DecodeError::Relation(
            "history cursor does not advance in its direction",
        ));
    }
    Ok(())
}

fn nodes_page(
    nodes: &[wire::HistoryNodeSummary],
    edges: &[wire::HistoryGraphEdge],
    identity: &wire::HistoryPageIdentity,
    request: &wire::HistoryWindowRequest,
) -> Result<(), DecodeError> {
    bounded(nodes.len(), request.limit.get())?;
    bounded(edges.len(), wire::MAX_HISTORY_PREVIEW_EDGES)?;
    ordered(
        nodes
            .iter()
            .map(|node| (std::cmp::Reverse(node.sequence), &node.node_id)),
    )?;
    let mut ids = BTreeSet::new();
    for node in nodes {
        if !ids.insert(&node.node_id)
            || node.current != (identity.context.current_node_id.as_ref() == Some(&node.node_id))
        {
            return Err(DecodeError::Relation(
                "history nodes repeat or misidentify the current node",
            ));
        }
        note_and_names(&node.note, &node.download_name, &node.storage_name)?;
        path(&node.path)?;
        if let Some(previous) = &node.previous_path {
            path(previous)?;
        }
        node_tags(&node.tags)?;
        if let wire::HistoryParentSet::More { cursor } = &node.parents {
            parent_cursor(cursor, &node.node_id, identity, request)?;
        }
    }
    if let Some(anchor) = &request.anchor_node_id
        && request.cursor.is_none()
        && !ids.contains(anchor)
    {
        return Err(DecodeError::Relation(
            "anchored history page omitted its anchor",
        ));
    }
    let mut pairs = BTreeSet::new();
    for edge in edges {
        let inside = ids.contains(&edge.parent_node_id);
        if !ids.contains(&edge.node_id)
            || edge.node_id == edge.parent_node_id
            || (edge.endpoint == wire::HistoryEndpoint::InWindow) != inside
            || !pairs.insert((&edge.node_id, &edge.parent_node_id))
        {
            return Err(DecodeError::Relation(
                "history edge endpoints disagree with the window",
            ));
        }
    }
    Ok(())
}

fn parent_cursor(
    cursor: &wire::HistoryCursor,
    node: &wire::HistoryNodeId,
    identity: &wire::HistoryPageIdentity,
    request: &wire::HistoryWindowRequest,
) -> Result<(), DecodeError> {
    let collection = wire::HistoryCollection::NodeParents {
        node_id: node.clone(),
    };
    let parent_request = wire::HistoryWindowRequest {
        file_id: request.file_id.clone(),
        collection,
        location: request.location.clone(),
        history_root: Some(identity.history_root),
        cursor: None,
        anchor_node_id: None,
        limit: cursor.payload().limit,
    };
    let parent_identity = wire::HistoryPageIdentity {
        history_root: identity.history_root,
        file_id: identity.file_id.clone(),
        collection: parent_request.collection.clone(),
        context: identity.context.clone(),
        query_hash: parent_request.query_hash(&identity.context, None),
        limit: parent_request.limit,
        action_max_entries: identity.action_max_entries,
    };
    wire::HistoryPageLimit::for_collection(parent_request.limit.get(), &parent_request.collection)
        .map_err(DecodeError::Name)?;
    response_cursor(
        cursor,
        wire::HistoryDirection::Forward,
        &parent_identity,
        &parent_request,
    )
}

fn versions_page(
    versions: &[wire::HistoryVersionSummary],
    limit: usize,
) -> Result<(), DecodeError> {
    bounded(versions.len(), limit)?;
    ordered(
        versions
            .iter()
            .map(|version| (version.version_sequence, &version.version_id)),
    )?;
    let mut ids = BTreeSet::new();
    for version in versions {
        if !ids.insert(&version.version_id) {
            return Err(DecodeError::Relation("history versions repeat an identity"));
        }
        note_and_names(&version.note, &version.download_name, &version.storage_name)?;
    }
    Ok(())
}

fn branches_page(
    branches: &[wire::HistoryBranchSummary],
    identity: &wire::HistoryPageIdentity,
    limit: usize,
) -> Result<(), DecodeError> {
    bounded(branches.len(), limit)?;
    ordered(branches.iter().map(|branch| &branch.branch_id))?;
    for branch in branches {
        storage_contract::validate_branch_name(&branch.name).map_err(DecodeError::Name)?;
        if branch.branch_kind != storage_contract::BranchKind::File
            || branch.current
                != (identity.context.selected_branch_id.as_ref() == Some(&branch.branch_id))
            || branch.read_only
                != (branch.state != storage_contract::BranchState::Active
                    || branch.alias_of.is_some())
        {
            return Err(DecodeError::Relation(
                "history branch disposition differs from its identity",
            ));
        }
        content_disposition(&branch.content)?;
    }
    Ok(())
}

fn content_disposition(content: &wire::HistoryContentDisposition) -> Result<(), DecodeError> {
    if let wire::HistoryContentSource::Inherited { source_version_id } = &content.source
        && source_version_id != &content.version_id
    {
        return Err(DecodeError::Relation(
            "inherited history content differs from its source",
        ));
    }
    if content.version_id.is_none() && content.stored_state.is_some() {
        return Err(DecodeError::Relation(
            "empty history content has a stored state",
        ));
    }
    Ok(())
}

fn merges_page(merges: &[wire::HistoryMergeSummary], limit: usize) -> Result<(), DecodeError> {
    bounded(merges.len(), limit)?;
    ordered(
        merges
            .iter()
            .map(|merge| (std::cmp::Reverse(merge.sequence), &merge.merge_id)),
    )?;
    let mut ids = BTreeSet::new();
    if merges.iter().any(|merge| !ids.insert(&merge.merge_id)) {
        return Err(DecodeError::Relation("history merges repeat an identity"));
    }
    Ok(())
}

fn bounded(count: usize, maximum: usize) -> Result<(), DecodeError> {
    if count > maximum {
        Err(DecodeError::Relation(
            "history collection exceeds its requested budget",
        ))
    } else {
        Ok(())
    }
}

fn ordered<T: Ord>(values: impl IntoIterator<Item = T>) -> Result<(), DecodeError> {
    let mut previous = None;
    for value in values {
        if previous.as_ref().is_some_and(|previous| previous >= &value) {
            return Err(DecodeError::Relation(
                "history records are duplicated or out of order",
            ));
        }
        previous = Some(value);
    }
    Ok(())
}

fn ordinals(values: impl IntoIterator<Item = u64>) -> Result<(), DecodeError> {
    let mut previous: Option<u64> = None;
    for value in values {
        if previous.is_some_and(|previous| previous.checked_add(1) != Some(value)) {
            return Err(DecodeError::Relation("history members are not contiguous"));
        }
        previous = Some(value);
    }
    Ok(())
}

fn note_and_names(note: &str, download: &str, stored: &str) -> Result<(), DecodeError> {
    storage_contract::validate_note(note).map_err(DecodeError::Name)?;
    for name in [download, stored] {
        name_policy::validate_name(name).map_err(DecodeError::Path)?;
    }
    Ok(())
}

fn path(value: &str) -> Result<(), DecodeError> {
    name_policy::validate_path(value).map_err(DecodeError::Path)
}

fn node_tags(tags: &[String]) -> Result<(), DecodeError> {
    bounded(tags.len(), storage_contract::MAX_FILE_TAGS)?;
    let mut seen = BTreeSet::new();
    for tag in tags {
        storage_contract::validate_tag(tag).map_err(DecodeError::Name)?;
        if !seen.insert(tag) {
            return Err(DecodeError::Relation("history node tags repeat"));
        }
    }
    Ok(())
}

struct FieldFragment<'a> {
    field: wire::HistoryField,
    total: u64,
    offset: u64,
    text: &'a str,
    end_of_field: bool,
}

fn fragment(
    fragment: FieldFragment<'_>,
    next: Option<&wire::HistoryCursor>,
    identity: &wire::HistoryPageIdentity,
    request: &wire::HistoryWindowRequest,
) -> Result<(), DecodeError> {
    let FieldFragment {
        field,
        total,
        offset,
        text,
        end_of_field,
    } = fragment;
    let wire::HistoryCollection::NodeField {
        field: requested, ..
    } = &request.collection
    else {
        return Err(DecodeError::Relation(
            "history fragment differs from its collection",
        ));
    };
    let expected_offset = match request
        .cursor
        .as_ref()
        .map(|cursor| &cursor.payload().position)
    {
        Some(wire::HistoryPosition::Field { byte_offset }) => *byte_offset,
        None => 0,
        Some(_) => {
            return Err(DecodeError::Relation(
                "history field cursor has no byte position",
            ));
        }
    };
    let end = offset
        .checked_add(text.len() as u64)
        .ok_or(DecodeError::Relation(
            "history fragment byte range overflows",
        ))?;
    if field != *requested
        || offset != expected_offset
        || text.len() > request.limit.get()
        || end > total
        || end_of_field != (end == total)
        || next.is_some() == end_of_field
        || text.is_empty() && !end_of_field
    {
        return Err(DecodeError::Relation(
            "history fragment has an inconsistent byte range",
        ));
    }
    if let Some(next) = next {
        response_cursor(next, wire::HistoryDirection::Forward, identity, request)?;
        if next.payload().position != (wire::HistoryPosition::Field { byte_offset: end }) {
            return Err(DecodeError::Relation(
                "history fragment cursor does not advance by its bytes",
            ));
        }
    }
    Ok(())
}

fn required_nullable(value: &Value, names: &[&str]) -> Result<(), DecodeError> {
    if let Some(object) = value.as_object()
        && names.iter().any(|name| !object.contains_key(*name))
    {
        return Err(DecodeError::Relation(
            "history response omitted a required nullable field",
        ));
    }
    Ok(())
}

fn nullable_content(value: &Value) -> Result<(), DecodeError> {
    required_nullable(value, &["version_id", "stored_state"])?;
    if let Some(source) = value.get("source")
        && source.get("kind").and_then(Value::as_str) == Some("inherited")
    {
        required_nullable(source, &["source_version_id"])?;
    }
    Ok(())
}

fn nullable_fields(value: &Value) -> Result<(), DecodeError> {
    if let Some(context) = value
        .get("identity")
        .and_then(|identity| identity.get("context"))
    {
        required_nullable(context, &["path", "current_node_id", "selected_branch_id"])?;
    }
    if let Some(page) = value.get("page") {
        required_nullable(page, &["previous_cursor", "next_cursor"])?;
    }
    if let Some(context) = value.get("context") {
        required_nullable(context, &["content"])?;
        if let Some(content) = context.get("content") {
            nullable_content(content)?;
        }
    }
    for collection in ["nodes", "versions", "branches"] {
        for record in value
            .get(collection)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match collection {
                "nodes" => {
                    required_nullable(record, &["branch_id", "previous_path", "content_state"])?
                }
                "versions" => required_nullable(record, &["content_state"])?,
                "branches" => {
                    required_nullable(record, &["node_id", "alias_of"])?;
                    if let Some(content) = record.get("content") {
                        nullable_content(content)?;
                    }
                }
                _ => unreachable!("invariant: history nullable field list is exhaustive"),
            }
        }
    }
    if value.get("kind").and_then(Value::as_str) == Some("node_field") {
        required_nullable(value, &["next_cursor"])?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/history/window_tests.rs"]
mod tests;
