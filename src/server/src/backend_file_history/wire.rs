//! Wire projections of bounded history pages and the byte-budgeted body encoder.
//!
//! Every list-bearing page is encoded record by record. The body is assembled
//! from admitted record bytes. Only node records whose edge membership changes
//! after truncation are re-encoded, still inside the same page byte budget.
use super::{
    AnchorPlacement, BoundedText, Continuation, FileBranchSummary, FileHistoryPage,
    FileMergeSummary, HistoryBranchesPage, HistoryContext, HistoryMergesPage, HistoryNodeKind,
    HistoryNodeSummary, IndexedSlice, NodeFieldSlice, NodeParentsPage, ParentPreview, VersionPage,
    limits,
};
use crate::ApiError;
use data_serialization::Serialize;
use http_routing::response::{IntoResponse, Response};
use storage_contract::client_protocol::{MAX_HISTORY_PAGE_BYTES, MAX_RESPONSE_BYTES};

/// Complete encoded page bound. The protocol value is a constant; tests may
/// shrink it through the test-only constructor to force short pages.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageBudget {
    max_bytes: usize,
}
impl PageBudget {
    pub(crate) const PROTOCOL: Self = Self {
        max_bytes: MAX_HISTORY_PAGE_BYTES,
    };
    #[cfg(test)]
    pub(crate) const fn with_max_bytes(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
    /// Shell reserve: two cursors, the head (context summary) and framing.
    fn reserve(self, head_bytes: usize) -> usize {
        2 * limits::MAX_CURSOR_CHARS + head_bytes + 1024
    }
}

/// An encoded JSON page body with the number of records it admitted.
#[derive(Debug)]
pub(crate) struct EncodedPage {
    body: Vec<u8>,
    #[cfg_attr(not(test), allow(dead_code, reason = "reported to byte-budget tests"))]
    admitted: usize,
}
struct ResponseLease {
    _permit: tokio::sync::OwnedSemaphorePermit,
    _deadline: Option<crate::http_shutdown::SendDeadline>,
}
struct LeasedChunk {
    bytes: Vec<u8>,
    _lease: std::sync::Arc<ResponseLease>,
}
impl AsRef<[u8]> for LeasedChunk {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl EncodedPage {
    pub(crate) fn into_leased_response(
        self,
        permit: tokio::sync::OwnedSemaphorePermit,
        limits: &crate::TransferLimits,
        deadline: tokio::time::Instant,
        connection: Option<crate::http_shutdown::ResponseConnection>,
    ) -> Response {
        let length = self.body.len();
        let lease = std::sync::Arc::new(ResponseLease {
            _permit: permit,
            _deadline: connection
                .as_ref()
                .map(|connection| connection.deadline(limits, deadline)),
        });
        let stream = futures::stream::try_unfold(
            (bytes::Bytes::from(self.body), lease),
            |(mut bytes, lease)| async move {
                if bytes.is_empty() {
                    return Ok::<_, std::io::Error>(None);
                }
                // Transport-owned chunks cannot pin the full page after
                // cancellation/timeout releases the response owner's permit.
                let chunk = bytes::Bytes::from_owner(LeasedChunk {
                    bytes: bytes.split_to(bytes.len().min(64 * 1024)).to_vec(),
                    _lease: lease.clone(),
                });
                Ok(Some((chunk, (bytes, lease))))
            },
        );
        let body = match connection {
            // The lease's socket-progress timer survives Body EOF and owns
            // the actual idle/total send boundary through the last chunk.
            Some(_) => http_routing::body::Body::from_stream(stream),
            // Embedded Router consumers have no socket/transport queue.
            None => http_routing::body::Body::from_stream(
                crate::transfer_limits::BoundedStream::new(stream, limits, deadline),
            ),
        };
        let mut response = body.into_response();
        response.headers_mut().insert(
            ::http::header::CONTENT_TYPE,
            ::http::HeaderValue::from_static("application/json"),
        );
        response.headers_mut().insert(
            ::http::header::CONTENT_LENGTH,
            ::http::HeaderValue::from_str(&length.to_string())
                .expect("invariant: decimal body length is a valid header"),
        );
        response
    }
    /// Records admitted by the byte budget (nearest the traversal boundary).
    #[cfg(test)]
    pub(crate) fn admitted(&self) -> usize {
        self.admitted
    }
    #[cfg(test)]
    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }
}
impl IntoResponse for EncodedPage {
    fn into_response(self) -> Response {
        (
            ::http::StatusCode::OK,
            [(::http::header::CONTENT_TYPE, "application/json")],
            self.body,
        )
            .into_response()
    }
}

/// A writer which refuses bytes before allocating beyond its admitted limit.
pub(crate) struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl BoundedWriter {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }

    pub(crate) fn json(&mut self, value: &impl Serialize) -> Result<(), ApiError> {
        match json_codec::to_writer(&mut *self, value) {
            Ok(()) => Ok(()),
            Err(_) if self.exceeded => Err(budget_error()),
            Err(source) => Err(ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::StateInvalid,
                "response encoding failed",
            )
            .with_source(source)),
        }
    }

    /// Only fixed JSON punctuation belongs here; all data goes through `json`.
    pub(crate) fn punctuation(&mut self, bytes: &'static [u8]) -> Result<(), ApiError> {
        std::io::Write::write_all(self, bytes).map_err(|_| budget_error())
    }

    pub(crate) fn finish(self) -> EncodedPage {
        EncodedPage {
            body: self.bytes,
            admitted: 1,
        }
    }
}
impl std::io::Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "history byte budget exhausted",
            ));
        }
        let needed = self.bytes.len() + bytes.len();
        if needed > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(needed)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_encoded(value: impl Serialize, limit: usize) -> Result<Option<Vec<u8>>, ApiError> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    match json_codec::to_writer(&mut writer, &value) {
        Ok(()) => Ok(Some(writer.bytes)),
        Err(_) if writer.exceeded => Ok(None),
        Err(source) => Err(ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::StateInvalid,
            "history response encoding failed",
        )
        .with_source(source)),
    }
}

/// Encode a bounded DTO once, retaining the same transport lease as list pages.
pub(crate) fn encode_bounded(value: impl Serialize) -> Result<EncodedPage, ApiError> {
    encode_with_limit(value, MAX_HISTORY_PAGE_BYTES)
}

/// Existing activity rows keep their fields; oversized responses fail intact.
pub(crate) fn encode_response_bounded(value: impl Serialize) -> Result<EncodedPage, ApiError> {
    encode_with_limit(value, MAX_RESPONSE_BYTES)
}

fn encode_with_limit(value: impl Serialize, limit: usize) -> Result<EncodedPage, ApiError> {
    let body = bounded_encoded(value, limit)?.ok_or_else(budget_error)?;
    Ok(EncodedPage { body, admitted: 1 })
}

fn budget_error() -> ApiError {
    ApiError::limit("history response record or envelope exceeds its byte budget")
}

/// Encode candidates lazily. At most the admitted records and one bounded
/// rejected candidate are visited; the first record never bypasses the cap.
fn prepare_records<R: Serialize, I: IntoIterator<Item = R>>(
    budget: PageBudget,
    reserve: usize,
    records: I,
    direction: super::PageDirection,
) -> Result<Vec<Vec<u8>>, ApiError>
where
    I::IntoIter: DoubleEndedIterator,
{
    let mut remaining = budget
        .max_bytes
        .checked_sub(reserve)
        .ok_or_else(budget_error)?;
    let mut admitted = Vec::new();
    let mut records = records.into_iter();
    while let Some(record) = match direction {
        super::PageDirection::Forward => records.next(),
        super::PageDirection::Backward => records.next_back(),
    } {
        let comma = usize::from(!admitted.is_empty());
        let candidate = match remaining.checked_sub(comma) {
            Some(limit) => bounded_encoded(record, limit)?,
            None => None,
        };
        let Some(bytes) = candidate else {
            if admitted.is_empty() {
                return Err(budget_error());
            }
            break;
        };
        remaining -= bytes.len() + comma;
        admitted.push(bytes);
    }
    if direction == super::PageDirection::Backward {
        admitted.reverse();
    }
    Ok(admitted)
}

fn assemble_page<T: Serialize>(
    budget: PageBudget,
    head: Vec<u8>,
    records_key: &str,
    records: Vec<Vec<u8>>,
    tail: T,
) -> Result<EncodedPage, ApiError> {
    let tail = bounded_encoded(tail, budget.max_bytes)?.ok_or_else(budget_error)?;
    let admitted = records.len();
    let used: usize = records.iter().map(Vec::len).sum();
    let length =
        head.len() + used + tail.len() + records_key.len() + 5 + admitted.saturating_sub(1);
    if length > budget.max_bytes {
        return Err(budget_error());
    }
    assert!(
        head.first() == Some(&b'{') && head.last() == Some(&b'}') && head.len() > 2,
        "invariant: page heads are non-empty JSON objects"
    );
    assert!(
        tail.first() == Some(&b'{') && tail.last() == Some(&b'}') && tail.len() > 2,
        "invariant: page tails are non-empty JSON objects"
    );
    let mut body = Vec::with_capacity(length);
    body.extend_from_slice(&head[..head.len() - 1]);
    body.extend_from_slice(b",\"");
    body.extend_from_slice(records_key.as_bytes());
    body.extend_from_slice(b"\":[");
    for (index, record) in records.into_iter().enumerate() {
        if index > 0 {
            body.push(b',');
        }
        body.extend_from_slice(&record);
    }
    body.extend_from_slice(b"],");
    body.extend_from_slice(&tail[1..]);
    debug_assert!(body.len() <= budget.max_bytes);
    Ok(EncodedPage { body, admitted })
}

pub(crate) fn encode_page<H: Serialize, T: Serialize, R: Serialize, I: IntoIterator<Item = R>>(
    budget: PageBudget,
    head: &H,
    records_key: &str,
    records: I,
    direction: super::PageDirection,
    tail: impl FnOnce(usize) -> Result<T, ApiError>,
) -> Result<EncodedPage, ApiError>
where
    I::IntoIter: DoubleEndedIterator,
{
    let head = bounded_encoded(head, budget.max_bytes)?.ok_or_else(budget_error)?;
    let records = prepare_records(budget, budget.reserve(head.len()), records, direction)?;
    let tail = tail(records.len())?;
    assemble_page(budget, head, records_key, records, tail)
}

fn serialize_content_state<S: data_serialization::Serializer>(
    state: &Option<storage_contract::ContentState>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(
        state
            .as_ref()
            .map(storage_contract::ContentState::as_str)
            .unwrap_or("LIVE"),
    )
}

#[derive(Serialize)]
struct BoundedTextResponse<'a> {
    bytes: usize,
    inline: Option<&'a str>,
}
impl<'a> From<&'a BoundedText> for BoundedTextResponse<'a> {
    fn from(text: &'a BoundedText) -> Self {
        Self {
            bytes: text.bytes,
            inline: text.inline.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct ParentEdgeResponse<'a> {
    node_id: &'a str,
    in_window: bool,
}

#[derive(Serialize)]
struct ParentPreviewResponse<'a> {
    status: &'static str,
    edges: Vec<ParentEdgeResponse<'a>>,
    cursor: Option<&'a str>,
}
impl<'a> From<&'a ParentPreview> for ParentPreviewResponse<'a> {
    fn from(preview: &'a ParentPreview) -> Self {
        let (status, edges, cursor) = match preview {
            ParentPreview::Complete(edges) => ("complete", edges, None),
            ParentPreview::More { edges, cursor } => ("more", edges, Some(cursor.as_str())),
        };
        Self {
            status,
            edges: edges
                .iter()
                .map(|edge| ParentEdgeResponse {
                    node_id: &edge.node_id,
                    in_window: edge.in_window,
                })
                .collect(),
            cursor,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct HistoryNodeResponse<'a> {
    node_id: &'a str,
    kind: &'static str,
    version_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    created_at: &'a str,
    branch_id: Option<&'a str>,
    path: &'a str,
    previous_path: Option<&'a str>,
    note: &'a str,
    tags: &'a std::collections::BTreeSet<String>,
    #[serialization(with = "storage_contract::u64_string")]
    size: u64,
    content_hash: storage_contract::ContentHash,
    #[serialization(serialize_with = "serialize_content_state")]
    content_state: Option<storage_contract::ContentState>,
    download_name: &'a str,
    storage_name: &'a str,
    operation_id: Option<storage_contract::OperationId>,
    author: BoundedTextResponse<'a>,
    message: BoundedTextResponse<'a>,
    parents: ParentPreviewResponse<'a>,
    current: bool,
}
impl<'a> HistoryNodeResponse<'a> {
    /// `current` marks exactly the context's current node, so a page never
    /// moves the current marker to its own newest node.
    fn new(node: &'a HistoryNodeSummary, current_node_id: Option<&str>) -> Self {
        Self {
            node_id: &node.node_id,
            kind: match node.kind {
                HistoryNodeKind::Content => "content",
                HistoryNodeKind::Rename => "rename",
                HistoryNodeKind::Tags => "tags",
                HistoryNodeKind::Merge => "merge",
            },
            version_id: &node.version_id,
            sequence: node.sequence,
            created_at: &node.created_at,
            branch_id: node.branch_id.as_deref(),
            path: &node.path,
            previous_path: node.previous_path.as_deref(),
            note: &node.note,
            tags: &node.tags,
            size: node.size,
            content_hash: node.content_hash,
            content_state: node.content_state,
            download_name: &node.download_name,
            storage_name: &node.storage_name,
            operation_id: node.operation_id,
            author: (&node.author).into(),
            message: (&node.message).into(),
            parents: (&node.parents).into(),
            current: current_node_id == Some(node.node_id.as_str()),
        }
    }
}

#[derive(Serialize)]
struct ContextResponse<'a> {
    path: Option<&'a str>,
    note: &'a str,
    current_node_id: Option<&'a str>,
    selected_branch_id: Option<&'a str>,
    #[serialization(flatten)]
    content: Option<crate::backend_branch::ContentDispositionResponse>,
}
impl<'a> From<&'a HistoryContext> for ContextResponse<'a> {
    fn from(context: &'a HistoryContext) -> Self {
        Self {
            path: context.path.as_deref(),
            note: &context.note,
            current_node_id: context.current_node_id.as_deref(),
            selected_branch_id: context.selected_branch_id.as_deref(),
            content: context.content.clone().map(Into::into),
        }
    }
}

#[derive(Serialize)]
struct NodePageHead<'a> {
    file_id: &'a str,
    repository_id: &'a str,
    history_tree_id: &'a str,
    history_root: &'a str,
    context: ContextResponse<'a>,
}

#[derive(Serialize)]
struct BudgetResponse {
    limit: usize,
    parent_edges: usize,
}

impl Serialize for AnchorPlacement {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use data_serialization::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("AnchorPlacement", 2)?;
        state.serialize_field("node_id", &self.node_id)?;
        state.serialize_field("in_page", &self.in_page)?;
        state.end()
    }
}

#[derive(Serialize)]
struct CollectionHead<'a> {
    file_id: &'a str,
    history_tree_id: &'a str,
    history_root: &'a str,
    #[serialization(skip_serializing_if = "Option::is_none")]
    node_id: Option<&'a str>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    selected_branch_id: Option<Option<&'a str>>,
}

#[derive(Serialize)]
struct CollectionTail {
    previous_cursor: Option<String>,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct ParentResponse<'a> {
    node_id: &'a str,
}

#[derive(Serialize)]
struct FileBranchSummaryResponse<'a> {
    branch_id: &'a str,
    history_tree_id: &'a str,
    name: &'a str,
    head_commit_id: &'a str,
    fork_commit_id: &'a str,
    branch_kind: storage_contract::BranchKind,
    file_id: &'a str,
    #[serialization(skip_serializing_if = "Option::is_none")]
    file_node_id: Option<&'a str>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    state: storage_contract::BranchState,
    #[serialization(flatten)]
    content: crate::backend_branch::ContentDispositionResponse,
    read_only: bool,
    alias_of: Option<&'a str>,
    node_id: Option<&'a str>,
    current: bool,
    pending_count: usize,
}
impl<'a> From<&'a FileBranchSummary> for FileBranchSummaryResponse<'a> {
    fn from(branch: &'a FileBranchSummary) -> Self {
        Self {
            branch_id: &branch.branch_id,
            history_tree_id: &branch.history_tree_id,
            name: &branch.name,
            head_commit_id: &branch.head_commit_id,
            fork_commit_id: &branch.fork_commit_id,
            branch_kind: branch.branch_kind,
            file_id: &branch.file_id,
            file_node_id: branch.file_node_id.as_deref(),
            generation: branch.generation,
            state: branch.state,
            content: branch.content.clone().into(),
            read_only: branch.read_only,
            alias_of: branch.alias_of.as_deref(),
            node_id: branch.node_id.as_deref(),
            current: branch.current,
            pending_count: branch.pending_count,
        }
    }
}

#[derive(Serialize)]
struct FileMergeSummaryResponse<'a> {
    merge_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    chosen_node_id: &'a str,
    result_node_id: &'a str,
    surviving_branch_id: &'a str,
    reused_node: bool,
    operation_id: storage_contract::OperationId,
    input_count: usize,
    parent_count: usize,
}
impl<'a> From<&'a FileMergeSummary> for FileMergeSummaryResponse<'a> {
    fn from(merge: &'a FileMergeSummary) -> Self {
        Self {
            merge_id: &merge.merge_id,
            sequence: merge.sequence,
            chosen_node_id: &merge.chosen_node_id,
            result_node_id: &merge.result_node_id,
            surviving_branch_id: &merge.surviving_branch_id,
            reused_node: merge.reused_node,
            operation_id: merge.operation_id,
            input_count: merge.input_count,
            parent_count: merge.parent_count,
        }
    }
}

#[derive(Serialize)]
struct SliceHead<'a> {
    file_id: &'a str,
    history_tree_id: &'a str,
    history_root: &'a str,
    #[serialization(skip_serializing_if = "Option::is_none")]
    merge_id: Option<&'a str>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    version_id: Option<&'a str>,
    offset: usize,
    total: usize,
}

#[derive(Serialize)]
struct SliceTail {
    next_offset: Option<usize>,
}

#[derive(Serialize)]
struct MergeInputResponse<'a> {
    index: usize,
    branch_id: &'a str,
    node_id: &'a str,
}

/// Mirrors `backend_read::VersionRecordResponse` without the unbounded
/// `source_version_ids`; `source_count` replaces it.
#[derive(Serialize)]
struct VersionSummaryResponse<'a> {
    history_tree_id: &'a str,
    version_id: &'a str,
    file_id: &'a str,
    creation_primary_slot_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    logical_size: u64,
    #[serialization(with = "storage_contract::u64_string")]
    version_sequence: u64,
    content_hash: String,
    checksum_chunk_bytes: u32,
    checksum_manifest_hash: &'a str,
    creation_name: &'a str,
    storage_name: &'a str,
    created_at: &'a str,
    created_as_full_checkpoint: bool,
    source_count: usize,
}
impl<'a> From<&'a super::VersionSummary> for VersionSummaryResponse<'a> {
    fn from(version: &'a super::VersionSummary) -> Self {
        let super::VersionSummary {
            history_tree_id,
            version_id,
            file_id,
            creation_primary_slot_id,
            logical_size,
            version_sequence,
            content_hash,
            checksum_chunk_bytes,
            checksum_manifest_hash,
            creation_name,
            storage_name,
            created_at,
            source_count,
            created_as_full_checkpoint,
        } = version;
        Self {
            history_tree_id,
            version_id,
            file_id,
            creation_primary_slot_id,
            logical_size: *logical_size,
            version_sequence: *version_sequence,
            content_hash: content_hash.to_string(),
            checksum_chunk_bytes: *checksum_chunk_bytes,
            checksum_manifest_hash,
            creation_name,
            storage_name,
            created_at,
            created_as_full_checkpoint: *created_as_full_checkpoint,
            source_count: *source_count,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct NodeFieldSliceResponse<'a> {
    file_id: &'a str,
    history_tree_id: &'a str,
    history_root: &'a str,
    node_id: &'a str,
    field: &'static str,
    total_bytes: usize,
    offset: usize,
    text: &'a str,
    complete: bool,
    next_offset: Option<usize>,
}
impl<'a> From<&'a NodeFieldSlice> for NodeFieldSliceResponse<'a> {
    fn from(slice: &'a NodeFieldSlice) -> Self {
        Self {
            file_id: &slice.file_id,
            history_tree_id: &slice.history_tree_id,
            history_root: &slice.history_root,
            node_id: &slice.node_id,
            field: slice.field.as_str(),
            total_bytes: slice.total_bytes,
            offset: slice.offset,
            text: &slice.text,
            complete: slice.complete,
            next_offset: (!slice.complete).then_some(slice.offset + slice.text.len()),
        }
    }
}

/// Retain the records nearest the requested traversal boundary: a canonical
/// prefix forward and suffix backward. The dropped side must stay reachable
/// by continuing in that same direction, even when byte page sizes differ.
fn continuation_cursors<K: Clone>(
    continuation: &Continuation,
    keys: &[K],
    admitted: usize,
    encode: impl Fn(bool, K) -> Result<String, ApiError>,
) -> Result<(Option<String>, Option<String>), ApiError> {
    let admitted_keys = &keys[continuation.direction.range(keys.len(), admitted)];
    let truncated = admitted < keys.len();
    let backward = continuation.direction == super::PageDirection::Backward;
    let previous = match (
        continuation.more_before || (truncated && backward),
        admitted_keys.first(),
    ) {
        (true, Some(first)) => Some(encode(true, first.clone())?),
        _ => None,
    };
    let next = match (
        continuation.more_after || (truncated && !backward),
        admitted_keys.last(),
    ) {
        (true, Some(last)) => Some(encode(false, last.clone())?),
        _ => None,
    };
    Ok((previous, next))
}

impl FileHistoryPage {
    /// Parent edges were marked `in_window` against the selected page. When the
    /// byte budget truncates the page, edges into dropped nodes become
    /// `outside_window`; each such flip adds exactly one byte ("false" vs
    /// "true"), so admission reserves one byte per possible page edge and only
    /// the affected admitted records are encoded again.
    pub(crate) fn encode(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let current = self.context.current_node_id.as_deref();
        let head = bounded_encoded(
            &NodePageHead {
                file_id: &self.file_id,
                repository_id: &self.repository_id,
                history_tree_id: &self.history_tree_id,
                history_root: &self.history_root,
                context: (&self.context).into(),
            },
            budget.max_bytes,
        )?
        .ok_or_else(budget_error)?;
        let mut records = prepare_records(
            budget,
            budget.reserve(head.len()) + limits::MAX_PAGE_PARENT_EDGES,
            self.nodes
                .iter()
                .map(|node| HistoryNodeResponse::new(node, current)),
            self.continuation.direction,
        )?;
        let admitted = records.len();
        let admitted_range = self
            .continuation
            .direction
            .range(self.nodes.len(), admitted);
        if admitted < self.nodes.len() {
            let resident = self.nodes[admitted_range.clone()]
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            for (node, record) in self.nodes[admitted_range.clone()].iter().zip(&mut records) {
                if node
                    .parents
                    .edges()
                    .iter()
                    .any(|edge| edge.in_window && !resident.contains(edge.node_id.as_str()))
                {
                    *record = bounded_encoded(
                        HistoryNodeResponse::new(&node.with_window(&resident), current),
                        record.len() + node.parents.edges().len(),
                    )?
                    .ok_or_else(budget_error)?;
                }
            }
            records.truncate(admitted);
        }
        let keys = self
            .nodes
            .iter()
            .map(|node| super::NodeCursorKey {
                sequence: node.sequence,
                node_id: node.node_id.clone(),
            })
            .collect::<Vec<_>>();
        let tail = {
            let (previous_cursor, next_cursor) =
                continuation_cursors(&self.continuation, &keys, admitted, |backward, key| {
                    super::NodeCursor::encode(&self.continuation.seed, backward, key)
                })?;
            let anchor = self.anchor.as_ref().map(|anchor| {
                // Budget truncation can move the anchor out of this page.
                let in_page = self.nodes[admitted_range.clone()]
                    .iter()
                    .any(|node| node.node_id == anchor.node_id);
                AnchorPlacement {
                    node_id: anchor.node_id.clone(),
                    in_page,
                }
            });
            NodePageTailOwned {
                previous_cursor,
                next_cursor,
                anchor,
                budget: BudgetResponse {
                    limit: self.limit,
                    parent_edges: limits::MAX_PAGE_PARENT_EDGES,
                },
            }
        };
        assemble_page(budget, head, "nodes", records, tail)
    }
}

#[derive(Serialize)]
struct NodePageTailOwned {
    previous_cursor: Option<String>,
    next_cursor: Option<String>,
    anchor: Option<AnchorPlacement>,
    budget: BudgetResponse,
}

impl NodeParentsPage {
    pub(crate) fn encode(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let records = self
            .parents
            .iter()
            .map(|parent| ParentResponse { node_id: parent });
        encode_page(
            budget,
            &CollectionHead {
                file_id: &self.file_id,
                history_tree_id: &self.history_tree_id,
                history_root: &self.history_root,
                node_id: Some(&self.node_id),
                selected_branch_id: None,
            },
            "parents",
            records,
            self.continuation.direction,
            |admitted| {
                let (previous_cursor, next_cursor) = continuation_cursors(
                    &self.continuation,
                    &self.parents,
                    admitted,
                    |backward, after| {
                        super::ParentCursor::encode(
                            &self.continuation.seed,
                            backward,
                            super::ParentCursorKey {
                                node_id: self.node_id.clone(),
                                after: Some(after),
                            },
                        )
                    },
                )?;
                Ok(CollectionTail {
                    previous_cursor,
                    next_cursor,
                })
            },
        )
    }
}

impl HistoryBranchesPage {
    pub(crate) fn encode(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let records = self.branches.iter().map(FileBranchSummaryResponse::from);
        let keys = self
            .branches
            .iter()
            .map(|branch| branch.branch_id.clone())
            .collect::<Vec<_>>();
        encode_page(
            budget,
            &CollectionHead {
                file_id: &self.file_id,
                history_tree_id: &self.history_tree_id,
                history_root: &self.history_root,
                node_id: None,
                selected_branch_id: Some(self.selected_branch_id.as_deref()),
            },
            "branches",
            records,
            self.continuation.direction,
            |admitted| {
                let (previous_cursor, next_cursor) = continuation_cursors(
                    &self.continuation,
                    &keys,
                    admitted,
                    |backward, after| {
                        super::BranchCursor::encode(
                            &self.continuation.seed,
                            backward,
                            super::BranchCursorKey { after },
                        )
                    },
                )?;
                Ok(CollectionTail {
                    previous_cursor,
                    next_cursor,
                })
            },
        )
    }
}

impl HistoryMergesPage {
    pub(crate) fn encode(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let records = self.merges.iter().map(FileMergeSummaryResponse::from);
        let keys = self
            .merges
            .iter()
            .map(|merge| super::MergeCursorKey {
                sequence: merge.sequence,
                merge_id: merge.merge_id.clone(),
            })
            .collect::<Vec<_>>();
        encode_page(
            budget,
            &CollectionHead {
                file_id: &self.file_id,
                history_tree_id: &self.history_tree_id,
                history_root: &self.history_root,
                node_id: None,
                selected_branch_id: None,
            },
            "merges",
            records,
            self.continuation.direction,
            |admitted| {
                let (previous_cursor, next_cursor) =
                    continuation_cursors(&self.continuation, &keys, admitted, |backward, key| {
                        super::MergeCursor::encode(&self.continuation.seed, backward, key)
                    })?;
                Ok(CollectionTail {
                    previous_cursor,
                    next_cursor,
                })
            },
        )
    }
}

impl VersionPage {
    pub(crate) fn encode(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let records = self.versions.iter().map(VersionSummaryResponse::from);
        let keys = self
            .versions
            .iter()
            .map(|version| super::VersionCursorKey {
                version_sequence: version.version_sequence,
                version_id: version.version_id.clone(),
            })
            .collect::<Vec<_>>();
        encode_page(
            budget,
            &CollectionHead {
                file_id: &self.file_id,
                history_tree_id: &self.history_tree_id,
                history_root: &self.history_root,
                node_id: None,
                selected_branch_id: None,
            },
            "versions",
            records,
            self.continuation.direction,
            |admitted| {
                let (previous_cursor, next_cursor) =
                    continuation_cursors(&self.continuation, &keys, admitted, |backward, key| {
                        super::VersionCursor::encode(&self.continuation.seed, backward, key)
                    })?;
                Ok(CollectionTail {
                    previous_cursor,
                    next_cursor,
                })
            },
        )
    }
}

impl<T> IndexedSlice<T> {
    fn encode_items<R: Serialize, I: IntoIterator<Item = R>>(
        &self,
        budget: PageBudget,
        subject: SliceSubject,
        key: &str,
        records: I,
    ) -> Result<EncodedPage, ApiError>
    where
        I::IntoIter: DoubleEndedIterator,
    {
        let (merge_id, version_id) = match subject {
            SliceSubject::Merge => (Some(self.subject_id.as_str()), None),
            SliceSubject::Version => (None, Some(self.subject_id.as_str())),
        };
        encode_page(
            budget,
            &SliceHead {
                file_id: &self.file_id,
                history_tree_id: &self.history_tree_id,
                history_root: &self.history_root,
                merge_id,
                version_id,
                offset: self.offset,
                total: self.total,
            },
            key,
            records,
            super::PageDirection::Forward,
            |admitted| {
                let end = self.offset + admitted;
                Ok(SliceTail {
                    next_offset: (end < self.total && admitted > 0).then_some(end),
                })
            },
        )
    }
}

#[derive(Clone, Copy)]
enum SliceSubject {
    Merge,
    Version,
}

impl IndexedSlice<(String, String)> {
    pub(crate) fn encode_inputs(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let records = self
            .items
            .iter()
            .enumerate()
            .map(|(position, (branch_id, node_id))| MergeInputResponse {
                index: self.offset + position,
                branch_id,
                node_id,
            });
        self.encode_items(budget, SliceSubject::Merge, "inputs", records)
    }
}

impl IndexedSlice<String> {
    pub(crate) fn encode_merge_parents(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let records =
            self.items
                .iter()
                .enumerate()
                .map(|(position, node_id)| IndexedNodeResponse {
                    index: self.offset + position,
                    node_id,
                });
        self.encode_items(budget, SliceSubject::Merge, "parents", records)
    }
    pub(crate) fn encode_sources(&self, budget: PageBudget) -> Result<EncodedPage, ApiError> {
        let records =
            self.items
                .iter()
                .enumerate()
                .map(|(position, version_id)| IndexedVersionResponse {
                    index: self.offset + position,
                    version_id,
                });
        self.encode_items(budget, SliceSubject::Version, "sources", records)
    }
}

#[derive(Serialize)]
struct IndexedNodeResponse<'a> {
    index: usize,
    node_id: &'a str,
}

#[derive(Serialize)]
struct IndexedVersionResponse<'a> {
    index: usize,
    version_id: &'a str,
}

impl NodeFieldSlice {
    /// A field slice is bounded by `max_bytes` (at most 1 MiB) plus a fixed
    /// shell; it is encoded in one piece below the page budget.
    pub(crate) fn encode(&self) -> Result<EncodedPage, ApiError> {
        let body = bounded_encoded(NodeFieldSliceResponse::from(self), MAX_HISTORY_PAGE_BYTES)?
            .ok_or_else(budget_error)?;
        Ok(EncodedPage { body, admitted: 1 })
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/backend_file_history/wire_tests.rs"]
mod tests;
