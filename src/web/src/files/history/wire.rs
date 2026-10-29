//! Strict decoding of the bounded file-history protocol (ADR 0009). Each DTO
//! rejects unknown fields; identities, cursors, roots and integers are parsed
//! into their types once, here, and never re-interpreted by consumers.
use super::{
    BoundedText, BranchPage, ContentDisposition, DecodeError, FieldSlice, FileBranch,
    HistoryBinding, HistoryContext, HistoryNodeSummary, HistoryRoot, HistoryWindow, MergeInput,
    MergeInputsPage, MergePage, MergeSummary, NodeField, NodeKind, ParentEdge, ParentPreview,
    ParentStatus, ParentsPage,
};
use data_serialization::{Deserialize, Deserializer};
use json_codec::Value;
use std::collections::BTreeSet;
use storage_contract::{BranchKind, BranchState};

const MAX_CURSOR_CHARS: usize = 32_768;
const MAX_IDENTITY_BYTES: usize = 256;

fn identity(value: &str) -> Result<(), DecodeError> {
    if value.is_empty() || value.len() > MAX_IDENTITY_BYTES {
        Err(DecodeError::Relation(
            "identities must be nonempty and at most 256 bytes",
        ))
    } else {
        Ok(())
    }
}
fn optional_identity(value: Option<&String>) -> Result<(), DecodeError> {
    value.map_or(Ok(()), |id| identity(id))
}
fn cursor(value: &Option<String>) -> Result<(), DecodeError> {
    match value {
        None => Ok(()),
        Some(cursor)
            if !cursor.is_empty()
                && cursor.len() <= MAX_CURSOR_CHARS
                && cursor.bytes().all(|byte| byte.is_ascii_graphic()) =>
        {
            Ok(())
        }
        Some(_) => Err(DecodeError::Relation(
            "cursors must be nonempty printable ASCII of at most 32768 characters",
        )),
    }
}
fn required_nullable<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
fn present_identity<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, DecodeError> {
    json_codec::from_value(value).map_err(DecodeError::Wire)
}

/// Shared page header: every bounded history response repeats it.
#[derive(Deserialize)]
struct Header {
    file_id: String,
    history_tree_id: String,
    history_root: String,
}
impl Header {
    fn check(&self, binding: &HistoryBinding) -> Result<(), DecodeError> {
        if self.file_id != binding.file_id {
            return Err(DecodeError::Relation(
                "page differs from the requested file",
            ));
        }
        if self.history_tree_id != binding.history_tree_id {
            return Err(DecodeError::Relation(
                "page differs from the resident history",
            ));
        }
        if HistoryRoot::parse(&self.history_root)? != binding.root {
            return Err(DecodeError::Relation(
                "page differs from the resident history root",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WindowWire {
    file_id: String,
    repository_id: String,
    history_tree_id: String,
    history_root: String,
    context: ContextWire,
    nodes: Vec<NodeWire>,
    #[serialization(deserialize_with = "required_nullable")]
    previous_cursor: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    next_cursor: Option<String>,
    anchor: Option<AnchorWire>,
    budget: BudgetWire,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct AnchorWire {
    node_id: String,
    in_page: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BudgetWire {
    limit: usize,
    parent_edges: usize,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ContextWire {
    #[serialization(deserialize_with = "required_nullable")]
    path: Option<String>,
    note: String,
    #[serialization(deserialize_with = "required_nullable")]
    current_node_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    selected_branch_id: Option<String>,
    // Content disposition fields are all present or all absent.
    #[serialization(default, deserialize_with = "present_nullable")]
    version_id: Option<Option<String>>,
    materialized: Option<bool>,
    inherited: Option<bool>,
    #[serialization(default, deserialize_with = "present_nullable")]
    own_version_id: Option<Option<String>>,
    #[serialization(default, deserialize_with = "present_nullable")]
    source_version_id: Option<Option<String>>,
    content_state: Option<String>,
    visible_content_state: Option<String>,
}
fn present_nullable<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct NodeWire {
    node_id: String,
    kind: KindWire,
    version_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    created_at: String,
    #[serialization(deserialize_with = "required_nullable")]
    branch_id: Option<String>,
    path: String,
    #[serialization(deserialize_with = "required_nullable")]
    previous_path: Option<String>,
    note: String,
    tags: Vec<String>,
    #[serialization(with = "storage_contract::u64_string")]
    size: u64,
    content_hash: String,
    content_state: String,
    download_name: String,
    storage_name: String,
    // Content nodes carry no operation; rename/metadata nodes always do.
    #[serialization(deserialize_with = "required_nullable")]
    operation_id: Option<String>,
    author: TextWire,
    message: TextWire,
    parents: ParentsWire,
    current: bool,
}
#[derive(Deserialize)]
#[serialization(rename_all = "snake_case")]
enum KindWire {
    Content,
    Rename,
    Tags,
    Merge,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct TextWire {
    bytes: usize,
    #[serialization(deserialize_with = "required_nullable")]
    inline: Option<String>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ParentsWire {
    status: ParentStatusWire,
    edges: Vec<EdgeWire>,
    #[serialization(deserialize_with = "required_nullable")]
    cursor: Option<String>,
}
#[derive(Deserialize, PartialEq)]
#[serialization(rename_all = "snake_case")]
enum ParentStatusWire {
    Complete,
    More,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct EdgeWire {
    node_id: String,
    in_window: bool,
}

fn text(wire: TextWire) -> Result<BoundedText, DecodeError> {
    let TextWire { bytes, inline } = wire;
    let inline_allowed = bytes <= storage_contract::client_protocol::MAX_INLINE_TEXT_BYTES;
    match &inline {
        Some(value) if value.len() != bytes || !inline_allowed => Err(DecodeError::Relation(
            "inline text must match its byte length within the inline bound",
        )),
        None if inline_allowed => Err(DecodeError::Relation(
            "texts within the inline bound must be inlined",
        )),
        _ => Ok(BoundedText { bytes, inline }),
    }
}

fn content_state(value: &str) -> Result<(), DecodeError> {
    if matches!(
        value,
        "LIVE" | "AVAILABLE" | "PURGED" | "MIGRATED" | "PENDING"
    ) {
        Ok(())
    } else {
        Err(DecodeError::Relation("unknown content state"))
    }
}

fn node(wire: NodeWire, page: &BTreeSet<&str>) -> Result<HistoryNodeSummary, DecodeError> {
    let NodeWire {
        node_id,
        kind,
        version_id,
        sequence,
        created_at,
        branch_id,
        path,
        previous_path,
        note,
        tags,
        size,
        content_hash,
        content_state: state,
        download_name,
        storage_name,
        operation_id,
        author,
        message,
        parents,
        current,
    } = wire;
    identity(&node_id)?;
    identity(&version_id)?;
    optional_identity(branch_id.as_ref())?;
    storage_contract::validate_note(&note).map_err(DecodeError::Name)?;
    if tags.len() > storage_contract::MAX_FILE_TAGS {
        return Err(DecodeError::Relation("a node carries at most 16 tags"));
    }
    let mut unique = BTreeSet::new();
    for tag in &tags {
        storage_contract::validate_tag(tag).map_err(DecodeError::Name)?;
        if !unique.insert(tag.clone()) {
            return Err(DecodeError::Relation("node tags repeat a label"));
        }
    }
    content_hash
        .parse::<storage_contract::ContentHash>()
        .map_err(|_| DecodeError::Relation("content_hash must be 64 lowercase hex"))?;
    content_state(&state)?;
    if let Some(operation_id) = &operation_id {
        operation_id
            .parse::<storage_contract::OperationId>()
            .map_err(DecodeError::Name)?;
    }
    if created_at.len() != 20 || !created_at.ends_with('Z') {
        return Err(DecodeError::Relation(
            "created_at must be a whole-second UTC timestamp",
        ));
    }
    let kind = match kind {
        KindWire::Content => NodeKind::Content,
        KindWire::Rename => NodeKind::Rename,
        KindWire::Tags => NodeKind::Tags,
        KindWire::Merge => NodeKind::Merge,
    };
    if kind == NodeKind::Content && node_id != version_id {
        return Err(DecodeError::Relation(
            "content nodes are identified by their body version",
        ));
    }
    if kind == NodeKind::Rename && previous_path.is_none() {
        return Err(DecodeError::Relation("rename nodes carry a previous path"));
    }
    let ParentsWire {
        status,
        edges,
        cursor: parent_cursor,
    } = parents;
    cursor(&parent_cursor)?;
    let status = match (status, parent_cursor) {
        (ParentStatusWire::Complete, None) => ParentStatus::Complete,
        (ParentStatusWire::More, Some(cursor)) => ParentStatus::More { cursor },
        _ => {
            return Err(DecodeError::Relation(
                "parent previews carry a cursor exactly when more parents exist",
            ));
        }
    };
    if edges.len() > 32 {
        return Err(DecodeError::Relation(
            "a parent preview enumerates at most 32 edges",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut parent_edges = Vec::with_capacity(edges.len());
    for EdgeWire {
        node_id: parent,
        in_window,
    } in edges
    {
        identity(&parent)?;
        if parent == node_id || !seen.insert(parent.clone()) {
            return Err(DecodeError::Relation(
                "parent edges are distinct and never self-referential",
            ));
        }
        if in_window != page.contains(parent.as_str()) {
            return Err(DecodeError::Relation(
                "in_window must describe this page's node set",
            ));
        }
        parent_edges.push(ParentEdge {
            node_id: parent,
            in_window,
        });
    }
    Ok(HistoryNodeSummary {
        node_id,
        kind,
        version_id,
        sequence,
        created_at,
        branch_id,
        path,
        previous_path,
        note,
        tags: unique,
        size,
        content_hash,
        content_state: state,
        download_name,
        storage_name,
        operation_id,
        author: text(author)?,
        message: text(message)?,
        parents: ParentPreview {
            status,
            edges: parent_edges,
        },
        current,
    })
}

fn context(wire: ContextWire) -> Result<(HistoryContext, Option<String>), DecodeError> {
    let ContextWire {
        path,
        note,
        current_node_id,
        selected_branch_id,
        version_id,
        materialized,
        inherited,
        own_version_id,
        source_version_id,
        content_state: state,
        visible_content_state,
    } = wire;
    optional_identity(current_node_id.as_ref())?;
    optional_identity(selected_branch_id.as_ref())?;
    let present = [
        version_id.is_some(),
        materialized.is_some(),
        inherited.is_some(),
        own_version_id.is_some(),
        source_version_id.is_some(),
        state.is_some(),
        visible_content_state.is_some(),
    ];
    let content = match (
        version_id,
        materialized,
        inherited,
        own_version_id,
        source_version_id,
        state,
        visible_content_state,
    ) {
        (
            Some(version_id),
            Some(materialized),
            Some(inherited),
            Some(own_version_id),
            Some(source_version_id),
            Some(content_state),
            Some(visible_content_state),
        ) => {
            optional_identity(version_id.as_ref())?;
            optional_identity(own_version_id.as_ref())?;
            let source = source_version_id.as_deref();
            if inherited != source.is_some()
                || materialized != (!inherited && version_id.is_some())
                || own_version_id.as_deref()
                    != if inherited {
                        None
                    } else {
                        version_id.as_deref()
                    }
                || !matches!(
                    visible_content_state.as_str(),
                    "LIVE" | "AVAILABLE" | "PURGED" | "MIGRATED"
                )
                || content_state
                    != if inherited {
                        "PENDING"
                    } else {
                        visible_content_state.as_str()
                    }
            {
                return Err(DecodeError::Relation(
                    "context content projection is inconsistent",
                ));
            }
            if path.is_none() {
                return Err(DecodeError::Relation(
                    "content disposition requires a current entry path",
                ));
            }
            Some(ContentDisposition {
                version_id,
                materialized,
                inherited,
                own_version_id,
                source_version_id,
                content_state,
                visible_content_state,
            })
        }
        _ if present.iter().any(|present| *present) => {
            return Err(DecodeError::Relation(
                "content disposition fields are all present or all absent",
            ));
        }
        _ => None,
    };
    if let Some(path) = &path {
        name_policy::validate_lookup_path(path)
            .map_err(|_| DecodeError::Relation("context path must be a valid logical path"))?;
    }
    Ok((
        HistoryContext {
            note,
            current_node_id,
            selected_branch_id,
            content,
        },
        path,
    ))
}

pub(super) fn window(value: Value, file: &str) -> Result<HistoryWindow, DecodeError> {
    let WindowWire {
        file_id,
        repository_id,
        history_tree_id,
        history_root,
        context: context_wire,
        nodes,
        previous_cursor,
        next_cursor,
        anchor,
        budget,
    } = decode(value)?;
    if file_id != file {
        return Err(DecodeError::Relation(
            "file history differs from the requested file",
        ));
    }
    identity(&file_id)?;
    identity(&history_tree_id)?;
    identity(&repository_id)?;
    let root = HistoryRoot::parse(&history_root)?;
    cursor(&previous_cursor)?;
    cursor(&next_cursor)?;
    if budget.limit == 0 || nodes.len() > budget.limit {
        return Err(DecodeError::Relation(
            "a page never exceeds its declared node limit",
        ));
    }
    let page = nodes
        .iter()
        .map(|node| node.node_id.clone())
        .collect::<BTreeSet<_>>();
    if page.len() != nodes.len() {
        return Err(DecodeError::Relation("page nodes repeat an identity"));
    }
    let page = page.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let (context, path) = context(context_wire)?;
    let mut summaries = Vec::with_capacity(nodes.len());
    let mut edges = 0usize;
    for wire in nodes {
        let summary = node(wire, &page)?;
        edges += summary.parents.edges.len();
        summaries.push(summary);
    }
    if edges > budget.parent_edges {
        return Err(DecodeError::Relation(
            "a page never exceeds its declared parent-edge budget",
        ));
    }
    for pair in summaries.windows(2) {
        let order = pair[1]
            .sequence
            .cmp(&pair[0].sequence)
            .then_with(|| pair[0].node_id.cmp(&pair[1].node_id));
        if order != std::cmp::Ordering::Less {
            return Err(DecodeError::Relation(
                "page nodes must follow (sequence DESC, node_id ASC)",
            ));
        }
    }
    let current = context.current_node_id.as_deref();
    if summaries
        .iter()
        .any(|node| node.current != (current == Some(node.node_id.as_str())))
    {
        return Err(DecodeError::Relation(
            "current markers must follow context.current_node_id",
        ));
    }
    let anchor = match anchor {
        None => None,
        Some(AnchorWire { node_id, in_page }) => {
            identity(&node_id)?;
            if in_page != page.contains(node_id.as_str()) {
                return Err(DecodeError::Relation(
                    "anchor.in_page must describe this page",
                ));
            }
            Some(node_id)
        }
    };
    Ok(HistoryWindow {
        binding: HistoryBinding {
            file_id,
            history_tree_id,
            root,
            repository_id,
            path,
        },
        context,
        nodes: summaries,
        previous_cursor,
        next_cursor,
        anchor,
    })
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BranchPageWire {
    #[serialization(flatten)]
    header: Header,
    #[serialization(deserialize_with = "required_nullable")]
    selected_branch_id: Option<String>,
    branches: Vec<BranchWire>,
    #[serialization(deserialize_with = "required_nullable")]
    previous_cursor: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BranchWire {
    branch_id: String,
    history_tree_id: String,
    name: String,
    head_commit_id: String,
    fork_commit_id: String,
    branch_kind: BranchKind,
    file_id: String,
    // The server omits this field when no explicit file node is recorded.
    #[serialization(default, deserialize_with = "present_identity")]
    file_node_id: Option<String>,
    #[serialization(rename = "generation", with = "storage_contract::u64_string")]
    _generation: u64,
    state: BranchState,
    materialized: bool,
    inherited: bool,
    content_state: String,
    #[serialization(deserialize_with = "required_nullable")]
    version_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    own_version_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    source_version_id: Option<String>,
    visible_content_state: String,
    read_only: bool,
    #[serialization(deserialize_with = "required_nullable")]
    alias_of: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    node_id: Option<String>,
    current: bool,
    pending_count: usize,
}

fn branch(
    wire: BranchWire,
    binding: &HistoryBinding,
    selected: Option<&str>,
) -> Result<FileBranch, DecodeError> {
    let BranchWire {
        branch_id,
        history_tree_id,
        name,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        _generation: _,
        state,
        materialized,
        inherited,
        content_state,
        version_id,
        own_version_id,
        source_version_id,
        visible_content_state,
        read_only,
        alias_of,
        node_id,
        current,
        pending_count,
    } = wire;
    for id in [
        &branch_id,
        &history_tree_id,
        &head_commit_id,
        &fork_commit_id,
        &file_id,
    ]
    .into_iter()
    .chain(file_node_id.iter())
    .chain(version_id.iter())
    .chain(own_version_id.iter())
    .chain(alias_of.iter())
    .chain(node_id.iter().filter(|id| !id.is_empty()))
    {
        identity(id)?;
    }
    if let Some(source) = source_version_id
        .as_ref()
        .filter(|source| !source.is_empty())
    {
        identity(source)?;
    }
    storage_contract::validate_branch_name(&name).map_err(DecodeError::Name)?;
    if branch_kind != BranchKind::File
        || file_id != binding.file_id
        || history_tree_id != binding.history_tree_id
    {
        return Err(DecodeError::Relation(
            "branch must belong to the requested file history",
        ));
    }
    if current != (selected == Some(branch_id.as_str())) {
        return Err(DecodeError::Relation(
            "current differs from selected_branch_id",
        ));
    }
    if read_only != (state != BranchState::Active || alias_of.is_some()) {
        return Err(DecodeError::Relation(
            "read_only differs from branch state or alias",
        ));
    }
    let source = source_version_id.as_deref();
    if inherited != source.is_some()
        || materialized != (!inherited && version_id.is_some())
        || own_version_id.as_deref()
            != if inherited {
                None
            } else {
                version_id.as_deref()
            }
        || inherited && version_id.as_deref() != source.filter(|id| !id.is_empty())
        || !matches!(
            visible_content_state.as_str(),
            "LIVE" | "AVAILABLE" | "PURGED" | "MIGRATED"
        )
        || content_state
            != if inherited {
                "PENDING"
            } else {
                visible_content_state.as_str()
            }
        || (pending_count == 0) == inherited
        || file_node_id
            .as_ref()
            .is_some_and(|id| node_id.as_ref() != Some(id))
    {
        return Err(DecodeError::Relation(
            "branch content projection is inconsistent",
        ));
    }
    Ok(FileBranch {
        id: branch_id,
        name,
        // An empty visible entry is an explicit current wire representation;
        // it is not a usable node identity inside the controls.
        head: node_id.filter(|id| !id.is_empty()).or(version_id),
        pending: !materialized,
        writable: !read_only,
    })
}

pub(super) fn branch_page(
    value: Value,
    binding: &HistoryBinding,
) -> Result<BranchPage, DecodeError> {
    let BranchPageWire {
        header,
        selected_branch_id,
        branches,
        previous_cursor,
        next_cursor,
    } = decode(value)?;
    header.check(binding)?;
    optional_identity(selected_branch_id.as_ref())?;
    cursor(&previous_cursor)?;
    cursor(&next_cursor)?;
    let mut records = Vec::with_capacity(branches.len());
    for wire in branches {
        let record = branch(wire, binding, selected_branch_id.as_deref())?;
        if records
            .last()
            .is_some_and(|previous: &FileBranch| previous.id >= record.id)
        {
            return Err(DecodeError::Relation(
                "branches must be distinct and ordered by branch_id",
            ));
        }
        records.push(record);
    }
    Ok(BranchPage {
        root: binding.root.clone(),
        branches: records,
        previous_cursor,
        next_cursor,
    })
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct MergePageWire {
    #[serialization(flatten)]
    header: Header,
    merges: Vec<MergeWire>,
    #[serialization(deserialize_with = "required_nullable")]
    previous_cursor: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct MergeWire {
    merge_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    chosen_node_id: String,
    result_node_id: String,
    surviving_branch_id: String,
    reused_node: bool,
    operation_id: String,
    input_count: usize,
    parent_count: usize,
}

pub(super) fn merge_page(value: Value, binding: &HistoryBinding) -> Result<MergePage, DecodeError> {
    let MergePageWire {
        header,
        merges,
        previous_cursor,
        next_cursor,
    } = decode(value)?;
    header.check(binding)?;
    cursor(&previous_cursor)?;
    cursor(&next_cursor)?;
    let mut records = Vec::with_capacity(merges.len());
    for MergeWire {
        merge_id,
        sequence,
        chosen_node_id,
        result_node_id,
        surviving_branch_id,
        reused_node,
        operation_id,
        input_count,
        parent_count,
    } in merges
    {
        for id in [
            &merge_id,
            &chosen_node_id,
            &result_node_id,
            &surviving_branch_id,
        ] {
            identity(id)?;
        }
        operation_id
            .parse::<storage_contract::OperationId>()
            .map_err(DecodeError::Name)?;
        if input_count < 2 {
            return Err(DecodeError::Relation("a merge records at least two inputs"));
        }
        if reused_node != (chosen_node_id == result_node_id) {
            return Err(DecodeError::Relation(
                "reused_node must describe whether the result is the chosen node",
            ));
        }
        let record = MergeSummary {
            merge_id,
            sequence,
            chosen_node_id,
            result_node_id,
            surviving_branch_id,
            reused_node,
            operation_id,
            input_count,
            parent_count,
        };
        // Canonical order: sequence DESC, merge_id ASC; equal keys repeat a record.
        if records.last().is_some_and(|previous: &MergeSummary| {
            previous
                .sequence
                .cmp(&record.sequence)
                .then_with(|| record.merge_id.cmp(&previous.merge_id))
                != std::cmp::Ordering::Greater
        }) {
            return Err(DecodeError::Relation(
                "merges must follow (sequence DESC, merge_id ASC)",
            ));
        }
        records.push(record);
    }
    Ok(MergePage {
        root: binding.root.clone(),
        merges: records,
        previous_cursor,
        next_cursor,
    })
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ParentsPageWire {
    #[serialization(flatten)]
    header: Header,
    node_id: String,
    parents: Vec<ParentWire>,
    #[serialization(deserialize_with = "required_nullable")]
    previous_cursor: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ParentWire {
    node_id: String,
}

pub(super) fn parents_page(
    value: Value,
    binding: &HistoryBinding,
    node: &str,
) -> Result<ParentsPage, DecodeError> {
    let ParentsPageWire {
        header,
        node_id,
        parents,
        previous_cursor,
        next_cursor,
    } = decode(value)?;
    header.check(binding)?;
    if node_id != node {
        return Err(DecodeError::Relation(
            "parents page differs from the requested node",
        ));
    }
    cursor(&previous_cursor)?;
    cursor(&next_cursor)?;
    let mut ids = Vec::with_capacity(parents.len());
    for ParentWire { node_id: parent } in parents {
        identity(&parent)?;
        if parent == node_id
            || ids
                .last()
                .is_some_and(|previous: &String| *previous >= parent)
        {
            return Err(DecodeError::Relation(
                "parents must be distinct, ascending and never the node itself",
            ));
        }
        ids.push(parent);
    }
    Ok(ParentsPage {
        node_id,
        parents: ids,
        previous_cursor,
        next_cursor,
    })
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct MergeInputsWire {
    #[serialization(flatten)]
    header: Header,
    merge_id: String,
    offset: usize,
    total: usize,
    inputs: Vec<MergeInputWire>,
    #[serialization(deserialize_with = "required_nullable_offset")]
    next_offset: Option<usize>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct MergeInputWire {
    index: usize,
    branch_id: String,
    node_id: String,
}
fn required_nullable_offset<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<usize>, D::Error> {
    Option::<usize>::deserialize(deserializer)
}

fn slice_bounds(
    offset: usize,
    len: usize,
    total: usize,
    next_offset: Option<usize>,
) -> Result<(), DecodeError> {
    let end = offset.checked_add(len).filter(|end| *end <= total);
    let Some(end) = end else {
        return Err(DecodeError::Relation("slice exceeds its declared total"));
    };
    if next_offset != (end < total).then_some(end) {
        return Err(DecodeError::Relation(
            "next_offset must continue exactly after this slice",
        ));
    }
    if len == 0 && end < total {
        return Err(DecodeError::Relation(
            "a non-final slice must make progress",
        ));
    }
    Ok(())
}

pub(super) fn merge_inputs(
    value: Value,
    binding: &HistoryBinding,
    merge: &str,
) -> Result<MergeInputsPage, DecodeError> {
    let MergeInputsWire {
        header,
        merge_id,
        offset,
        total,
        inputs,
        next_offset,
    } = decode(value)?;
    header.check(binding)?;
    if merge_id != merge {
        return Err(DecodeError::Relation(
            "inputs page differs from the requested merge",
        ));
    }
    slice_bounds(offset, inputs.len(), total, next_offset)?;
    let mut records = Vec::with_capacity(inputs.len());
    for (position, wire) in inputs.into_iter().enumerate() {
        let MergeInputWire {
            index,
            branch_id,
            node_id,
        } = wire;
        identity(&branch_id)?;
        identity(&node_id)?;
        if index != offset + position {
            return Err(DecodeError::Relation(
                "merge inputs must carry their original consecutive index",
            ));
        }
        records.push(MergeInput {
            index,
            branch_id,
            node_id,
        });
    }
    Ok(MergeInputsPage {
        merge_id,
        offset,
        total,
        inputs: records,
        next_offset,
    })
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct FieldSliceWire {
    #[serialization(flatten)]
    header: Header,
    node_id: String,
    field: FieldWire,
    total_bytes: usize,
    offset: usize,
    text: String,
    complete: bool,
    #[serialization(deserialize_with = "required_nullable_offset")]
    next_offset: Option<usize>,
}
#[derive(Deserialize, PartialEq)]
#[serialization(rename_all = "snake_case")]
enum FieldWire {
    Message,
    Author,
}

pub(super) fn field_slice(
    value: Value,
    binding: &HistoryBinding,
    node: &str,
    field: NodeField,
) -> Result<FieldSlice, DecodeError> {
    let FieldSliceWire {
        header,
        node_id,
        field: wire_field,
        total_bytes,
        offset,
        text,
        complete,
        next_offset,
    } = decode(value)?;
    header.check(binding)?;
    let decoded = match wire_field {
        FieldWire::Message => NodeField::Message,
        FieldWire::Author => NodeField::Author,
    };
    if node_id != node || decoded != field {
        return Err(DecodeError::Relation(
            "field slice differs from the requested node field",
        ));
    }
    slice_bounds(offset, text.len(), total_bytes, next_offset)?;
    if complete != (offset + text.len() == total_bytes) {
        return Err(DecodeError::Relation(
            "complete is true exactly when the slice ends the field",
        ));
    }
    Ok(FieldSlice {
        node_id,
        field,
        total_bytes,
        offset,
        text,
        complete,
        next_offset,
    })
}
