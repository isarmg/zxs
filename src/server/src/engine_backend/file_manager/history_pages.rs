//! Bounded file-history pages over the engine's page projection.
//!
//! File/root admission is locked; detached resource owners perform node,
//! branch, parent and field projection outside the writer lock. Version and
//! merge selection retain their existing plans. JSON encoding is unlocked.
use super::*;
use crate::backend_activity::utc_timestamp;
use crate::backend_file_history::{
    AnchorPlacement, BoundedText, Continuation, CursorKind, FileBranchSummary, FileHistoryPage,
    FileMergeSummary, HistoryBranchesPage, HistoryContext, HistoryCursorSeed, HistoryMergesPage,
    HistoryNodeKind, HistoryNodeSummary, HistoryRootText, IndexedSlice, NodeCursor, NodeFieldName,
    NodeFieldSlice, NodeParentsPage, NodePosition, ParentCursor, ParentCursorKey, ParentEdge,
    ParentPreview, ReadFileHistory, ReadHistoryBranches, ReadHistoryMerges, ReadIndexedSlice,
    ReadNodeField, ReadNodeParents, ReadVersionPage, VersionPage, limits,
};
use storage_engine::file_history_pages::{
    ContextLocation, Direction, FileHistoryScope, MergeKey, NodeField, NodeKey, NodeKind, NodeTime,
    Selection, VersionKey,
};

/// The admitted scope of one request: repository, exact root and projection.
struct Admitted<'a> {
    repo: &'a Repository,
    root: storage_engine::HistoryRoot,
    scope: FileHistoryScope<'a>,
}

/// Resolve the file's repository, capture the current root, and refuse a
/// request bound to another root. Object authorization (`file_repo`) runs
/// before the root comparison so an unauthorized object never leaks change.
fn admit_root<'a>(
    engine: &'a Engine,
    file: &'a str,
    observed: Option<&HistoryRootText>,
) -> Result<(&'a Repository, storage_engine::HistoryRoot), ApiError> {
    let repo = file_repo(engine, file)?;
    let root = engine
        .history_root(&repo.history_tree_id)
        .map_err(super::activity::read_error)?;
    if let Some(observed) = observed
        && observed.as_str() != root.as_str()
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::HistoryChanged,
            "file history changed; reload the first page or re-anchor",
        ));
    }
    Ok((repo, root))
}

fn admit<'a>(
    engine: &'a Engine,
    file: &'a str,
    observed: Option<&HistoryRootText>,
) -> Result<Admitted<'a>, ApiError> {
    let (repo, root) = admit_root(engine, file, observed)?;
    let scope = engine
        .file_history_scope(file, &repo.history_tree_id)
        .map_err(super::activity::read_error)?;
    Ok(Admitted { repo, root, scope })
}

/// Captured authorization context is only a projection source, never authority.
pub(in crate::engine_backend) struct FileHistoryCapture {
    view: storage_engine::FileHistoryReadView,
    root: storage_engine::HistoryRoot,
    file: String,
    repository: String,
}

pub(in crate::engine_backend) fn capture(
    engine: &Engine,
    file: &str,
    observed: Option<&HistoryRootText>,
) -> Result<FileHistoryCapture, ApiError> {
    let (repo, root) = admit_root(engine, file, observed)?;
    Ok(FileHistoryCapture {
        view: engine
            .file_history_read_view()
            .map_err(super::activity::read_error)?,
        root,
        file: file.to_owned(),
        repository: repo.repository_id.clone(),
    })
}

impl FileHistoryCapture {
    fn admitted<'a>(
        &'a self,
        observe: &'a dyn Fn() -> storage_engine::Result<()>,
    ) -> Result<Admitted<'a>, ApiError> {
        let repo = self
            .view
            .state()
            .repositories
            .get(&self.repository)
            .expect("invariant: captured repository belongs to captured State");
        let scope = self
            .view
            .file_history_scope(&self.file, &repo.history_tree_id, observe)
            .map_err(super::activity::read_error)?;
        Ok(Admitted {
            repo,
            root: self.root.clone(),
            scope,
        })
    }

    pub(in crate::engine_backend) fn file_history(
        self,
        request: ReadFileHistory,
        control: &crate::HistoryReadControl,
    ) -> Result<FileHistoryPage, ApiError> {
        let observe = || control.checkpoint().map_err(storage_engine::Error::from);
        file_history(self.admitted(&observe)?, request)
    }
    pub(in crate::engine_backend) fn node_parents(
        self,
        request: ReadNodeParents,
        control: &crate::HistoryReadControl,
    ) -> Result<NodeParentsPage, ApiError> {
        let observe = || control.checkpoint().map_err(storage_engine::Error::from);
        node_parents(self.admitted(&observe)?, request)
    }
    pub(in crate::engine_backend) fn branches(
        self,
        request: ReadHistoryBranches,
        control: &crate::HistoryReadControl,
    ) -> Result<HistoryBranchesPage, ApiError> {
        let observe = || control.checkpoint().map_err(storage_engine::Error::from);
        branches(self.admitted(&observe)?, request)
    }
    pub(in crate::engine_backend) fn node_field(
        self,
        request: ReadNodeField,
        control: &crate::HistoryReadControl,
    ) -> Result<NodeFieldSlice, ApiError> {
        let observe = || control.checkpoint().map_err(storage_engine::Error::from);
        node_field(self.admitted(&observe)?, request)
    }
}

fn seed(admitted: &Admitted<'_>, kind: CursorKind, request: &Value) -> HistoryCursorSeed {
    HistoryCursorSeed::new(
        kind,
        admitted.root.as_str(),
        &admitted.repo.history_tree_id,
        admitted.scope.file_id(),
        request,
    )
}

fn continuation<K>(
    seed: HistoryCursorSeed,
    selection: &Selection<K>,
    backward: bool,
) -> Continuation {
    Continuation {
        direction: crate::backend_file_history::PageDirection::from_backward(backward),
        seed,
        more_before: selection.more_before,
        more_after: selection.more_after,
    }
}

fn node_time(time: &NodeTime) -> Result<String, ApiError> {
    match time {
        NodeTime::Rfc3339(text) => Ok(text.clone()),
        NodeTime::UnixSeconds(seconds) => utc_timestamp(*seconds),
    }
}

fn bounded(text: storage_engine::file_history_pages::BoundedText) -> BoundedText {
    BoundedText {
        bytes: text.bytes,
        inline: text.inline,
    }
}

fn node_kind(kind: NodeKind) -> HistoryNodeKind {
    match kind {
        NodeKind::Content => HistoryNodeKind::Content,
        NodeKind::Rename => HistoryNodeKind::Rename,
        NodeKind::Tags => HistoryNodeKind::Tags,
        NodeKind::Merge => HistoryNodeKind::Merge,
    }
}

/// The current entry of the file for this request's context rule.
fn context(
    admitted: &Admitted<'_>,
    location: Option<(&str, &name_policy::LookupPath)>,
) -> Result<(HistoryContext, Value), ApiError> {
    let repo = admitted.repo;
    let resolved = match location {
        Some((repository_id, path)) => {
            if repository_id != repo.repository_id {
                return Err(ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::FileNotFound,
                    "file is not present in this repository context",
                ));
            }
            let context = admitted
                .scope
                .current_context(ContextLocation::PathKey(path.key()))
                .map_err(super::activity::read_error)?;
            if context.current.is_none() {
                return Err(ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::FileNotFound,
                    "file is not present at this logical path",
                ));
            }
            context
        }
        None => admitted
            .scope
            .current_context(ContextLocation::Resolve)
            .map_err(super::activity::read_error)?,
    };
    // The resolved context binds every cursor of this page: a continuation
    // cannot silently resolve to another alias of the same file.
    let identity = json!({
        "requested": location.map(|(repository, path)| json!({"repository_id":repository,"path":path.key()})),
        "repository_id": resolved.repository_id,
        "path": resolved.path,
        "current_node_id": resolved.current_node_id,
    });
    let content = resolved.current.as_ref().map(|current| {
        super::super::branch::capture_content(
            admitted.scope_state(),
            Some(&current.version_id),
            current.source_version_id.as_deref(),
        )
    });
    Ok((
        HistoryContext {
            path: resolved.path,
            note: resolved.note,
            current_node_id: resolved.current_node_id,
            selected_branch_id: resolved.selected_branch_id,
            content,
        },
        identity,
    ))
}

impl<'a> Admitted<'a> {
    fn scope_state(&self) -> &'a storage_contract::State {
        self.scope.state()
    }
}

fn boundary_key(cursor: &NodeCursor) -> (NodeKey, Direction) {
    (
        NodeKey {
            sequence: cursor.key.sequence,
            node_id: cursor.key.node_id.clone(),
        },
        if cursor.backward {
            Direction::Backward
        } else {
            Direction::Forward
        },
    )
}

fn file_history(
    admitted: Admitted<'_>,
    request: ReadFileHistory,
) -> Result<FileHistoryPage, ApiError> {
    let ReadFileHistory {
        file_id: _,
        location,
        limit,
        position,
        root: _,
    } = request;
    let (context, identity) = context(
        &admitted,
        location
            .as_ref()
            .map(|location| (location.repository_id.as_str(), &location.path)),
    )?;
    let seed = seed(
        &admitted,
        CursorKind::Nodes,
        &json!({"context":identity,"limit":limit,"order":"sequence-desc-node-id-asc","budget":"history-page-v1"}),
    );
    let scope = &admitted.scope;
    let backward = matches!(&position, NodePosition::Cursor(cursor) if cursor.backward);
    let (selection, anchor) = match position {
        NodePosition::First => (
            scope
                .select_nodes(None, limit)
                .map_err(super::activity::read_error)?,
            None,
        ),
        NodePosition::Cursor(cursor) => {
            cursor.seed.admit(&seed)?;
            let (key, direction) = boundary_key(&cursor);
            if scope
                .node_key(&key.node_id)
                .map_err(super::activity::read_error)?
                != key
            {
                return Err(ApiError::invalid(
                    "cursor boundary sequence does not match its node",
                ));
            }
            (
                scope
                    .select_nodes(Some((&key, direction)), limit)
                    .map_err(super::activity::read_error)?,
                None,
            )
        }
        NodePosition::Anchor(anchor) => {
            let key = scope
                .node_key(anchor.as_str())
                .map_err(super::activity::read_error)?;
            // Position the page so the anchor is its first node: the key just
            // before the anchor is the exclusive forward boundary.
            let before = scope
                .select_nodes(Some((&key, Direction::Backward)), 1)
                .map_err(super::activity::read_error)?;
            let start = before.keys.first();
            let selection = scope
                .select_nodes(start.map(|key| (key, Direction::Forward)), limit)
                .map_err(super::activity::read_error)?;
            let in_page = selection
                .keys
                .iter()
                .any(|key| key.node_id == anchor.as_str());
            (
                selection,
                Some(AnchorPlacement {
                    node_id: anchor.as_str().to_owned(),
                    in_page,
                }),
            )
        }
    };
    let page_ids = selection
        .keys
        .iter()
        .map(|key| key.node_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut reader = scope.node_reader();
    let mut edges_left = limits::MAX_PAGE_PARENT_EDGES;
    let parent_seed = HistoryCursorSeed::new(
        CursorKind::Parents,
        admitted.root.as_str(),
        &admitted.repo.history_tree_id,
        scope.file_id(),
        &json!({"limit":limits::PARENTS_DEFAULT,"order":"parent-id-asc"}),
    );
    let mut nodes = Vec::with_capacity(selection.keys.len());
    for key in &selection.keys {
        let record = reader.record(key).map_err(super::activity::read_error)?;
        let preview_limit = edges_left.min(limits::MAX_PARENT_PREVIEW_PER_NODE);
        let parents = if preview_limit == 0 {
            // No budget left: still an honest, continuable parent state.
            ParentPreview::More {
                edges: Vec::new(),
                cursor: ParentCursor::encode(
                    &parent_seed,
                    false,
                    ParentCursorKey {
                        node_id: key.node_id.clone(),
                        after: None,
                    },
                )?,
            }
        } else {
            let parents = reader
                .parents(&key.node_id, None, preview_limit)
                .map_err(super::activity::read_error)?;
            edges_left -= parents.keys.len();
            let edges = parents
                .keys
                .iter()
                .map(|parent| ParentEdge {
                    node_id: parent.clone(),
                    in_window: page_ids.contains(parent.as_str()),
                })
                .collect::<Vec<_>>();
            if parents.more_after {
                let last = parents
                    .keys
                    .last()
                    .expect("invariant: a continuable parent page is non-empty")
                    .clone();
                ParentPreview::More {
                    edges,
                    cursor: ParentCursor::encode(
                        &parent_seed,
                        false,
                        ParentCursorKey {
                            node_id: key.node_id.clone(),
                            after: Some(last),
                        },
                    )?,
                }
            } else {
                ParentPreview::Complete(edges)
            }
        };
        nodes.push(HistoryNodeSummary {
            node_id: record.node_id,
            kind: node_kind(record.kind),
            version_id: record.version_id,
            sequence: record.sequence,
            created_at: node_time(&record.created_at)?,
            branch_id: record.branch_id,
            path: record.path,
            previous_path: record.previous_path,
            note: record.note,
            tags: record.tags,
            size: record.size,
            content_hash: record.content_hash,
            content_state: record.content_state,
            download_name: record.download_name,
            storage_name: record.storage_name,
            operation_id: record.operation_id,
            author: bounded(record.author),
            message: bounded(record.message),
            parents,
        });
    }
    Ok(FileHistoryPage {
        file_id: scope.file_id().to_owned(),
        repository_id: admitted.repo.repository_id.clone(),
        history_tree_id: admitted.repo.history_tree_id.clone(),
        history_root: admitted.root.as_str().to_owned(),
        context,
        nodes,
        continuation: continuation(seed, &selection, backward),
        anchor,
        limit,
    })
}

fn node_parents(
    admitted: Admitted<'_>,
    request: ReadNodeParents,
) -> Result<NodeParentsPage, ApiError> {
    let ReadNodeParents {
        node_id,
        collection,
    } = request;
    let seed = seed(
        &admitted,
        CursorKind::Parents,
        &json!({"limit":collection.limit,"order":"parent-id-asc"}),
    );
    let after = match &collection.cursor {
        Some(cursor) => {
            // Node-page previews are minted with the default limit; a
            // continuation may use another limit as long as the scope matches.
            cursor.seed.admit_scope(&seed)?;
            if cursor.key.node_id != node_id.as_str() {
                return Err(ApiError::invalid("cursor does not match the history query"));
            }
            if cursor.backward && cursor.key.after.is_none() {
                return Err(ApiError::invalid(
                    "backward parent cursor requires a boundary",
                ));
            }
            cursor.key.after.as_deref().map(|key| {
                (
                    key,
                    if cursor.backward {
                        Direction::Backward
                    } else {
                        Direction::Forward
                    },
                )
            })
        }
        None => None,
    };
    let selection = admitted
        .scope
        .node_parent_window(node_id.as_str(), after, collection.limit)
        .map_err(super::activity::read_error)?;
    Ok(NodeParentsPage {
        file_id: admitted.scope.file_id().to_owned(),
        history_tree_id: admitted.repo.history_tree_id.clone(),
        history_root: admitted.root.as_str().to_owned(),
        node_id: node_id.as_str().to_owned(),
        parents: selection.keys.clone(),
        continuation: continuation(
            seed,
            &selection,
            collection
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.backward),
        ),
    })
}

fn branches(
    admitted: Admitted<'_>,
    request: ReadHistoryBranches,
) -> Result<HistoryBranchesPage, ApiError> {
    let collection = request.0;
    let seed = seed(
        &admitted,
        CursorKind::Branches,
        &json!({"limit":collection.limit,"order":"branch-id-asc"}),
    );
    let after = match &collection.cursor {
        Some(cursor) => {
            cursor.seed.admit(&seed)?;
            admitted
                .scope
                .branch_record(&cursor.key.after)
                .map_err(super::activity::read_error)?;
            Some((
                cursor.key.after.as_str(),
                if cursor.backward {
                    Direction::Backward
                } else {
                    Direction::Forward
                },
            ))
        }
        None => None,
    };
    let scope = &admitted.scope;
    let selection = scope
        .branch_window(after, collection.limit)
        .map_err(super::activity::read_error)?;
    let selected = scope
        .current_context(ContextLocation::Resolve)
        .map_err(super::activity::read_error)?
        .selected_branch_id;
    let state = admitted.scope_state();
    let branches = selection
        .keys
        .iter()
        .map(|id| {
            let record = scope
                .branch_record(id)
                .map_err(super::activity::read_error)?;
            Ok(FileBranchSummary {
                current: selected.as_deref() == Some(record.branch_id.as_str()),
                content: super::super::branch::capture_content(
                    state,
                    record.visible_version_id.as_deref(),
                    record.source_version_id.as_deref(),
                ),
                branch_id: record.branch_id,
                history_tree_id: record.history_tree_id,
                name: record.name,
                head_commit_id: record.head_commit_id,
                fork_commit_id: record.fork_commit_id,
                branch_kind: record.branch_kind,
                file_id: scope.file_id().to_owned(),
                file_node_id: record.file_node_id,
                generation: record.generation,
                state: record.state,
                read_only: record.read_only,
                alias_of: record.alias_of,
                node_id: record.node_id,
                pending_count: record.pending_count,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(HistoryBranchesPage {
        file_id: scope.file_id().to_owned(),
        history_tree_id: admitted.repo.history_tree_id.clone(),
        history_root: admitted.root.as_str().to_owned(),
        selected_branch_id: selected,
        branches,
        continuation: continuation(
            seed,
            &selection,
            collection
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.backward),
        ),
    })
}

pub(in crate::engine_backend) struct MergePagePlan {
    file_id: String,
    history_tree_id: String,
    history_root: String,
    seed: HistoryCursorSeed,
    backward: bool,
    merges: storage_engine::file_history_pages::FileMergeReadPlan,
}

pub(in crate::engine_backend) fn prepare_merge_page(
    engine: &Engine,
    request: ReadHistoryMerges,
) -> Result<MergePagePlan, ApiError> {
    let collection = request.0;
    let admitted = admit(engine, collection.file_id.as_str(), Some(&collection.root))?;
    let seed = seed(
        &admitted,
        CursorKind::Merges,
        &json!({"limit":collection.limit,"order":"sequence-desc-merge-id-asc"}),
    );
    let boundary = match &collection.cursor {
        Some(cursor) => {
            cursor.seed.admit(&seed)?;
            Some((
                MergeKey {
                    sequence: cursor.key.sequence,
                    merge_id: cursor.key.merge_id.clone(),
                },
                if cursor.backward {
                    Direction::Backward
                } else {
                    Direction::Forward
                },
            ))
        }
        None => None,
    };
    Ok(MergePagePlan {
        file_id: admitted.scope.file_id().to_owned(),
        history_tree_id: admitted.repo.history_tree_id.clone(),
        history_root: admitted.root.as_str().to_owned(),
        seed,
        backward: collection
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.backward),
        merges: admitted
            .scope
            .prepare_merge_page(boundary, collection.limit)
            .map_err(super::activity::read_error)?,
    })
}

impl MergePagePlan {
    pub(in crate::engine_backend) fn read(self) -> HistoryMergesPage {
        let (selection, records) = self.merges.read();
        let merges = records
            .into_iter()
            .map(|record| FileMergeSummary {
                merge_id: record.merge_id,
                sequence: record.sequence,
                chosen_node_id: record.chosen_node_id,
                result_node_id: record.result_node_id,
                surviving_branch_id: record.surviving_branch_id,
                reused_node: record.reused_node,
                operation_id: record.operation_id,
                input_count: record.input_count,
                parent_count: record.parent_count,
            })
            .collect();
        HistoryMergesPage {
            file_id: self.file_id,
            history_tree_id: self.history_tree_id,
            history_root: self.history_root,
            merges,
            continuation: continuation(self.seed, &selection, self.backward),
        }
    }
}

fn indexed<T>(
    engine: &Engine,
    request: &ReadIndexedSlice,
    read: impl FnOnce(
        &FileHistoryScope<'_>,
        &str,
        usize,
        usize,
    ) -> storage_engine::Result<storage_engine::file_history_pages::Slice<T>>,
) -> Result<IndexedSlice<T>, ApiError> {
    let admitted = admit(engine, request.file_id.as_str(), Some(&request.root))?;
    let slice = read(
        &admitted.scope,
        request.subject_id.as_str(),
        request.offset,
        request.limit,
    )
    .map_err(super::activity::read_error)?;
    Ok(IndexedSlice {
        file_id: admitted.scope.file_id().to_owned(),
        history_tree_id: admitted.repo.history_tree_id.clone(),
        history_root: admitted.root.as_str().to_owned(),
        subject_id: request.subject_id.as_str().to_owned(),
        offset: slice.offset,
        total: slice.total,
        items: slice.items,
    })
}

pub(in crate::engine_backend) fn merge_inputs(
    engine: &Engine,
    request: &ReadIndexedSlice,
) -> Result<IndexedSlice<(String, String)>, ApiError> {
    indexed(engine, request, |scope, merge, offset, limit| {
        scope.merge_inputs(merge, offset, limit)
    })
}

pub(in crate::engine_backend) fn merge_parents(
    engine: &Engine,
    request: &ReadIndexedSlice,
) -> Result<IndexedSlice<String>, ApiError> {
    indexed(engine, request, |scope, merge, offset, limit| {
        scope.merge_parents(merge, offset, limit)
    })
}

pub(in crate::engine_backend) fn version_sources(
    engine: &Engine,
    request: &ReadIndexedSlice,
) -> Result<IndexedSlice<String>, ApiError> {
    indexed(engine, request, |scope, version, offset, limit| {
        scope.version_sources(version, offset, limit)
    })
}

fn node_field(admitted: Admitted<'_>, request: ReadNodeField) -> Result<NodeFieldSlice, ApiError> {
    // One raw byte can become six JSON bytes (\u00xx). Reserve 8 KiB for
    // the fixed shell, bounded identities and numeric fields before copying
    // any field text; the encoder independently bounds the complete body.
    const MAX_ENCODE_SAFE_FIELD_BYTES: usize =
        (storage_contract::client_protocol::MAX_HISTORY_PAGE_BYTES - 8192) / 6;
    let field = match request.field {
        NodeFieldName::Message => NodeField::Message,
        NodeFieldName::Author => NodeField::Author,
    };
    let slice = admitted
        .scope
        .node_field(
            request.node_id.as_str(),
            field,
            request.offset,
            request.max_bytes.min(MAX_ENCODE_SAFE_FIELD_BYTES),
        )
        .map_err(super::activity::read_error)?;
    Ok(NodeFieldSlice {
        file_id: admitted.scope.file_id().to_owned(),
        history_tree_id: admitted.repo.history_tree_id.clone(),
        history_root: admitted.root.as_str().to_owned(),
        node_id: request.node_id.as_str().to_owned(),
        field: request.field,
        total_bytes: slice.total_bytes,
        offset: slice.offset,
        text: slice.text,
        complete: slice.complete,
    })
}

pub(in crate::engine_backend) struct VersionPagePlan {
    file_id: String,
    history_tree_id: String,
    history_root: String,
    seed: HistoryCursorSeed,
    backward: bool,
    versions: storage_engine::file_history_pages::FileVersionReadPlan,
}

pub(in crate::engine_backend) fn prepare_version_page(
    engine: &Engine,
    request: ReadVersionPage,
) -> Result<VersionPagePlan, ApiError> {
    let collection = request;
    let admitted = admit(
        engine,
        collection.file_id.as_str(),
        collection.root.as_ref(),
    )?;
    let seed = seed(
        &admitted,
        CursorKind::Versions,
        &json!({"limit":collection.limit,"order":"version-sequence-asc-version-id-asc"}),
    );
    let boundary = match &collection.cursor {
        Some(cursor) => {
            cursor.seed.admit(&seed)?;
            Some((
                VersionKey {
                    version_sequence: cursor.key.version_sequence,
                    version_id: cursor.key.version_id.clone(),
                },
                if cursor.backward {
                    Direction::Backward
                } else {
                    Direction::Forward
                },
            ))
        }
        None => None,
    };
    Ok(VersionPagePlan {
        file_id: admitted.scope.file_id().to_owned(),
        history_tree_id: admitted.repo.history_tree_id.clone(),
        history_root: admitted.root.as_str().to_owned(),
        seed,
        backward: collection
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.backward),
        versions: admitted
            .scope
            .prepare_version_page(boundary, collection.limit)
            .map_err(super::activity::read_error)?,
    })
}

impl VersionPagePlan {
    pub(in crate::engine_backend) fn read(self) -> Result<VersionPage, ApiError> {
        let (selection, records) = self.versions.read();
        if selection.keys.is_empty() && !selection.more_before && !selection.more_after {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "file history not found",
            ));
        }
        let versions = records
            .iter()
            .map(|record| crate::backend_file_history::VersionSummary::from(record.as_ref()))
            .collect();
        Ok(VersionPage {
            file_id: self.file_id,
            history_tree_id: self.history_tree_id,
            history_root: self.history_root,
            versions,
            continuation: continuation(self.seed, &selection, self.backward),
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/history_pages_tests.rs"]
mod tests;
