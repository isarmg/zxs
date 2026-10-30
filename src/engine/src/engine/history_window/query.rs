//! Root/query admission and finite stable-key collection windows.
use super::*;
use nodes::Scope;
use selection::{Position, Selection, Selector};
use std::cmp::Reverse;

fn invalid(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::InvalidRequest, message)
}

pub(super) fn changed() -> Error {
    Error::new(
        storage_contract::ErrorCode::HistoryChanged,
        "history or its resolved path context changed; refresh the current history window",
    )
}

pub(super) struct Query<'a> {
    pub(super) state: &'a State,
    pub(super) request: &'a HistoryWindowRequest,
    pub(super) context: &'a HistoryResolvedContext,
    pub(super) cancel: &'a HistoryWindowCancellation,
    pub(super) identity: HistoryPageIdentity,
    anchor: Option<&'a HistoryNodeId>,
}

impl MetadataView {
    /// Project a finite page from this already admitted observation. Callers
    /// supply a context resolved under current object/path authorization and
    /// must recheck it before publishing the result. No body is opened.
    ///
    /// Added space is O(page records + preview edges + field bytes), bounded by
    /// shared policy constants. Cold selection scans State; node commit/path
    /// and first-parent facts can additionally scan commits and ancestor chains.
    pub(crate) fn history_window(
        &self,
        request: &HistoryWindowRequest,
        context: &HistoryResolvedContext,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<HistoryWindowPage> {
        Query::new(self, request, context, cancellation)?.page()
    }
}

impl<'a> Query<'a> {
    pub(super) fn new(
        view: &'a MetadataView,
        request: &'a HistoryWindowRequest,
        context: &'a HistoryResolvedContext,
        cancel: &'a HistoryWindowCancellation,
    ) -> Result<Self> {
        cancel.checkpoint()?;
        validate_request(request)?;
        let state = view.state();
        validate_scope(state, request, context, cancel)?;
        let root = HistoryRoot::parse(view.history_root(context.history_tree_id.as_str())?)?;
        let (anchor, query_hash) = validate_cursor(state, request, context, root)?;
        Ok(Self {
            state,
            request,
            context,
            cancel,
            identity: HistoryPageIdentity {
                history_root: root,
                file_id: request.file_id.clone(),
                collection: request.collection.clone(),
                context: context.clone(),
                query_hash,
                limit: request.limit,
                action_max_entries: u64::try_from(view.history_action_max_entries)?,
            },
            anchor,
        })
    }

    pub(super) fn scope(&self) -> Scope<'a> {
        Scope {
            file: self.request.file_id.as_str(),
            history: self.context.history_tree_id.as_str(),
        }
    }

    fn page(&self) -> Result<HistoryWindowPage> {
        match &self.request.collection {
            HistoryCollection::Nodes => self.node_page(),
            HistoryCollection::Versions => self.version_page(),
            HistoryCollection::Branches => self.branch_page(),
            HistoryCollection::Merges => self.merge_page(),
            HistoryCollection::NodeParents { node_id } => self.parent_page(node_id.as_str()),
            HistoryCollection::VersionSources { version_id } => {
                self.source_page(version_id.as_str())
            }
            HistoryCollection::MergeInputs { merge_id } => self.input_page(merge_id.as_str()),
            HistoryCollection::MergeParents { merge_id } => {
                self.merge_parent_page(merge_id.as_str())
            }
            HistoryCollection::NodeField { node_id, field } => {
                self.field_page(node_id.as_str(), *field)
            }
        }
    }

    pub(super) fn cursor(
        &self,
        identity: &HistoryPageIdentity,
        direction: HistoryDirection,
        position: HistoryPosition,
        anchor: Option<&HistoryNodeId>,
    ) -> Result<HistoryCursor> {
        Ok(HistoryCursor::from_payload(HistoryCursorPayload {
            format: HistoryCursorFormat::HistoryWindowV1,
            history_root: identity.history_root,
            history_tree_id: identity.context.history_tree_id.clone(),
            file_id: identity.file_id.clone(),
            collection: identity.collection.clone(),
            query_hash: identity.query_hash,
            limit: identity.limit,
            direction,
            position,
            anchor_node_id: anchor.cloned(),
        })?)
    }

    pub(super) fn auxiliary_identity(
        &self,
        collection: HistoryCollection,
        limit: HistoryPageLimit,
    ) -> HistoryPageIdentity {
        let request = HistoryWindowRequest {
            file_id: self.request.file_id.clone(),
            collection: collection.clone(),
            location: self.request.location.clone(),
            history_root: Some(self.identity.history_root),
            cursor: None,
            anchor_node_id: None,
            limit,
        };
        HistoryPageIdentity {
            history_root: self.identity.history_root,
            file_id: request.file_id.clone(),
            collection,
            context: self.context.clone(),
            query_hash: request.query_hash(self.context, None),
            limit,
            action_max_entries: self.identity.action_max_entries,
        }
    }

    fn position<K>(
        &self,
        decode: impl FnOnce(&'a HistoryPosition) -> Result<K>,
    ) -> Result<Position<K>> {
        let Some(cursor) = &self.request.cursor else {
            return Ok(Position::Start);
        };
        if cursor.payload().position == HistoryPosition::Start {
            return Ok(Position::Start);
        }
        let key = decode(&cursor.payload().position)?;
        Ok(match cursor.payload().direction {
            HistoryDirection::Forward => Position::After(key),
            HistoryDirection::Backward => Position::Before(key),
        })
    }

    fn page_cursors<K, T>(
        &self,
        selection: &Selection<K, T>,
        encode: impl Fn(&K) -> Result<HistoryPosition>,
    ) -> Result<HistoryPageCursors> {
        let previous_cursor = match selection.entries.first().filter(|_| selection.has_before) {
            Some((key, _)) => Some(self.cursor(
                &self.identity,
                HistoryDirection::Backward,
                encode(key)?,
                self.anchor,
            )?),
            None => None,
        };
        let next_cursor = match selection.entries.last().filter(|_| selection.has_after) {
            Some((key, _)) => Some(self.cursor(
                &self.identity,
                HistoryDirection::Forward,
                encode(key)?,
                self.anchor,
            )?),
            None => None,
        };
        Ok(HistoryPageCursors {
            previous_cursor,
            next_cursor,
        })
    }

    fn node_page(&self) -> Result<HistoryWindowPage> {
        let scope = self.scope();
        let position = if let Some(anchor) = self.request.anchor_node_id.as_ref() {
            let node = self.node(anchor.as_str())?;
            let commit = nodes::first_commit(
                self.state,
                scope,
                &node.version(self.state).version_id,
                self.cancel,
            )?;
            Position::At((Reverse(node.sequence(commit)), node.id()))
        } else {
            self.position(|position| match position {
                HistoryPosition::Node { sequence, node_id } => {
                    Ok((Reverse(*sequence), node_id.as_str()))
                }
                _ => Err(invalid("node cursor position required")),
            })?
        };
        let mut selector = Selector::new(position, self.request.limit.get())?;
        for candidate in nodes::all_nodes(self.state, scope, self.cancel) {
            let node = candidate?;
            let commit = nodes::first_commit(
                self.state,
                scope,
                &node.version(self.state).version_id,
                self.cancel,
            )?;
            selector.offer((Reverse(node.sequence(commit)), node.id()), (node, commit));
        }
        let selection = selector.finish()?;
        let page = self.page_cursors(&selection, |(Reverse(sequence), id)| {
            Ok(HistoryPosition::Node {
                sequence: *sequence,
                node_id: HistoryNodeId::parse(id)?,
            })
        })?;
        let mut records = Vec::with_capacity(selection.entries.len());
        let mut edges = Vec::new();
        for (index, (_, (node, commit))) in selection.entries.iter().enumerate() {
            self.cancel.checkpoint()?;
            // At most 64 nodes share 256 edges. Reserve one preview for each
            // later node, so More always has a real enumerated boundary key.
            let remaining_nodes = selection.entries.len() - index - 1;
            let preview_limit = MAX_HISTORY_PREVIEW_EDGES - edges.len() - remaining_nodes;
            let parents = self.parents(node.id(), Position::Start, preview_limit)?;
            let parent_set = if parents.has_after {
                let last = parents
                    .entries
                    .last()
                    .expect("invariant: nonempty parent preview advances before More")
                    .0;
                let collection = HistoryCollection::NodeParents {
                    node_id: HistoryNodeId::parse(node.id())?,
                };
                let identity = self.auxiliary_identity(
                    collection.clone(),
                    HistoryPageLimit::default_for(&collection),
                );
                HistoryParentSet::More {
                    cursor: self.cursor(
                        &identity,
                        HistoryDirection::Forward,
                        HistoryPosition::Parent {
                            parent_node_id: HistoryNodeId::parse(last)?,
                        },
                        None,
                    )?,
                }
            } else {
                HistoryParentSet::Complete
            };
            for (parent, ()) in parents.entries {
                let endpoint = if selection
                    .entries
                    .iter()
                    .any(|(_, (node, _))| node.id() == parent)
                {
                    HistoryEndpoint::InWindow
                } else {
                    HistoryEndpoint::OutsideWindow
                };
                edges.push(HistoryGraphEdge {
                    node_id: HistoryNodeId::parse(node.id())?,
                    parent_node_id: HistoryNodeId::parse(parent)?,
                    endpoint,
                });
            }
            records.push(self.node_summary(*node, *commit, parent_set)?);
        }
        Ok(HistoryWindowPage::Nodes {
            identity: self.identity.clone(),
            context: self.context_summary()?,
            nodes: records,
            edges,
            page,
        })
    }

    fn version_page(&self) -> Result<HistoryWindowPage> {
        let position = self.position(|position| match position {
            HistoryPosition::Version {
                sequence,
                version_id,
            } => Ok((*sequence, version_id.as_str())),
            _ => Err(invalid("version cursor position required")),
        })?;
        let mut selector = Selector::new(position, self.request.limit.get())?;
        for version in self.state.versions.values() {
            self.cancel.checkpoint()?;
            if version.file_id == self.scope().file
                && version.history_tree_id == self.scope().history
            {
                selector.offer(
                    (version.version_sequence, version.version_id.as_str()),
                    version,
                );
            }
        }
        let selection = selector.finish()?;
        let page = self.page_cursors(&selection, |(sequence, id)| {
            Ok(HistoryPosition::Version {
                sequence: *sequence,
                version_id: HistoryVersionId::parse(id)?,
            })
        })?;
        let versions = selection
            .entries
            .iter()
            .map(|(_, version)| self.version_summary(version))
            .collect::<Result<_>>()?;
        Ok(HistoryWindowPage::Versions {
            identity: self.identity.clone(),
            versions,
            page,
        })
    }

    fn branch_page(&self) -> Result<HistoryWindowPage> {
        let position = self.position(|position| match position {
            HistoryPosition::Branch { branch_id } => Ok(branch_id.as_str()),
            _ => Err(invalid("branch cursor position required")),
        })?;
        let mut selector = Selector::new(position, self.request.limit.get())?;
        for branch in self.state.branches.values() {
            self.cancel.checkpoint()?;
            if branch.file_id.as_deref() == Some(self.scope().file)
                && branch.history_tree_id == self.scope().history
                && branch.state != crate::BranchState::Migrated
            {
                selector.offer(branch.branch_id.as_str(), branch);
            }
        }
        let selection = selector.finish()?;
        let page = self.page_cursors(&selection, |id| {
            Ok(HistoryPosition::Branch {
                branch_id: HistoryBranchId::parse(id)?,
            })
        })?;
        let branches = selection
            .entries
            .iter()
            .map(|(_, branch)| self.branch_summary(branch))
            .collect::<Result<_>>()?;
        Ok(HistoryWindowPage::Branches {
            identity: self.identity.clone(),
            branches,
            page,
        })
    }

    fn merge_page(&self) -> Result<HistoryWindowPage> {
        let position = self.position(|position| match position {
            HistoryPosition::Merge { sequence, merge_id } => {
                Ok((Reverse(*sequence), merge_id.as_str()))
            }
            _ => Err(invalid("merge cursor position required")),
        })?;
        let mut selector = Selector::new(position, self.request.limit.get())?;
        for merge in self.state.file_branch_merges.values() {
            self.cancel.checkpoint()?;
            if merge.file_id == self.scope().file && merge.history_tree_id == self.scope().history {
                selector.offer((Reverse(merge.sequence), merge.merge_id.as_str()), merge);
            }
        }
        let selection = selector.finish()?;
        let page = self.page_cursors(&selection, |(Reverse(sequence), id)| {
            Ok(HistoryPosition::Merge {
                sequence: *sequence,
                merge_id: HistoryMergeId::parse(id)?,
            })
        })?;
        let merges = selection
            .entries
            .iter()
            .map(|(_, merge)| self.merge_summary(merge))
            .collect::<Result<_>>()?;
        Ok(HistoryWindowPage::Merges {
            identity: self.identity.clone(),
            merges,
            page,
        })
    }

    fn parents<'b>(
        &'b self,
        id: &str,
        position: Position<&'b str>,
        limit: usize,
    ) -> Result<Selection<&'b str, ()>> {
        self.node(id)?;
        let mut selector = Selector::new(position, limit)?;
        nodes::visit_parents(self.state, self.scope(), id, self.cancel, |parent| {
            selector.offer(parent, ());
            Ok(())
        })?;
        selector.finish()
    }

    fn parent_page(&self, id: &str) -> Result<HistoryWindowPage> {
        let position = self.position(|position| match position {
            HistoryPosition::Parent { parent_node_id } => Ok(parent_node_id.as_str()),
            _ => Err(invalid("parent cursor position required")),
        })?;
        let selection = self.parents(id, position, self.request.limit.get())?;
        let page = self.page_cursors(&selection, |id| {
            Ok(HistoryPosition::Parent {
                parent_node_id: HistoryNodeId::parse(id)?,
            })
        })?;
        let parents = selection
            .entries
            .iter()
            .map(|(id, ())| {
                Ok(HistoryParentMember {
                    parent_node_id: HistoryNodeId::parse(id)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(HistoryWindowPage::NodeParents {
            identity: self.identity.clone(),
            parents,
            page,
        })
    }

    fn member_position(&self) -> Result<Position<u64>> {
        self.position(|position| match position {
            HistoryPosition::Member { ordinal } => Ok(*ordinal),
            _ => Err(invalid("member cursor position required")),
        })
    }

    fn member_cursors<T>(&self, selection: &Selection<u64, T>) -> Result<HistoryPageCursors> {
        self.page_cursors(selection, |ordinal| {
            Ok(HistoryPosition::Member { ordinal: *ordinal })
        })
    }

    fn source_page(&self, id: &str) -> Result<HistoryWindowPage> {
        let version = self.version(id)?;
        let mut selector = Selector::new(self.member_position()?, self.request.limit.get())?;
        for (ordinal, source) in version.source_version_ids.iter().enumerate() {
            self.cancel.checkpoint()?;
            if self.allowed_version(source) {
                selector.offer(u64::try_from(ordinal)?, source.as_str());
            }
        }
        let selection = selector.finish()?;
        let page = self.member_cursors(&selection)?;
        let sources = selection
            .entries
            .iter()
            .map(|(ordinal, id)| {
                Ok(HistorySourceMember {
                    ordinal: *ordinal,
                    version_id: HistoryVersionId::parse(id)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(HistoryWindowPage::VersionSources {
            identity: self.identity.clone(),
            sources,
            page,
        })
    }

    fn input_page(&self, id: &str) -> Result<HistoryWindowPage> {
        let merge = self.merge(id)?;
        let mut selector = Selector::new(self.member_position()?, self.request.limit.get())?;
        for (ordinal, (branch, node)) in merge
            .input_branch_ids
            .iter()
            .zip(&merge.input_node_ids)
            .enumerate()
        {
            self.cancel.checkpoint()?;
            if self.allowed_branch(branch) && nodes::node(self.state, self.scope(), node).is_some()
            {
                selector.offer(u64::try_from(ordinal)?, (branch.as_str(), node.as_str()));
            }
        }
        let selection = selector.finish()?;
        let page = self.member_cursors(&selection)?;
        let inputs = selection
            .entries
            .iter()
            .map(|(ordinal, (branch, node))| {
                Ok(HistoryMergeInputMember {
                    ordinal: *ordinal,
                    branch_id: HistoryBranchId::parse(branch)?,
                    node_id: HistoryNodeId::parse(node)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(HistoryWindowPage::MergeInputs {
            identity: self.identity.clone(),
            inputs,
            page,
        })
    }

    fn merge_parent_page(&self, id: &str) -> Result<HistoryWindowPage> {
        let merge = self.merge(id)?;
        let mut selector = Selector::new(self.member_position()?, self.request.limit.get())?;
        for (ordinal, parent) in merge.parent_node_ids.iter().enumerate() {
            self.cancel.checkpoint()?;
            if nodes::node(self.state, self.scope(), parent).is_some() {
                selector.offer(u64::try_from(ordinal)?, parent.as_str());
            }
        }
        let selection = selector.finish()?;
        let page = self.member_cursors(&selection)?;
        let parents = selection
            .entries
            .iter()
            .map(|(ordinal, id)| {
                Ok(HistoryMergeParentMember {
                    ordinal: *ordinal,
                    parent_node_id: HistoryNodeId::parse(id)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(HistoryWindowPage::MergeParents {
            identity: self.identity.clone(),
            parents,
            page,
        })
    }

    fn field_page(&self, id: &str, field: HistoryField) -> Result<HistoryWindowPage> {
        let node = self.node(id)?;
        let text = self.field(node, field)?;
        let offset = match &self.request.cursor {
            Some(cursor) => match (&cursor.payload().position, cursor.payload().direction) {
                (HistoryPosition::Field { byte_offset }, HistoryDirection::Forward) => {
                    usize::try_from(*byte_offset)?
                }
                _ => return Err(invalid("field cursor requires a forward UTF-8 byte offset")),
            },
            None => 0,
        };
        if offset > text.len() || !text.is_char_boundary(offset) {
            return Err(invalid("field offset is outside a UTF-8 boundary"));
        }
        self.cancel.checkpoint()?;
        let mut end = offset + self.request.limit.get().min(text.len() - offset);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset && end < text.len() {
            return Err(invalid(
                "field byte budget cannot fit its next UTF-8 character",
            ));
        }
        let next_cursor = if end < text.len() {
            Some(self.cursor(
                &self.identity,
                HistoryDirection::Forward,
                HistoryPosition::Field {
                    byte_offset: u64::try_from(end)?,
                },
                None,
            )?)
        } else {
            None
        };
        let fragment = text[offset..end].to_owned();
        self.cancel.checkpoint()?;
        Ok(HistoryWindowPage::NodeField {
            identity: self.identity.clone(),
            field,
            total_bytes: u64::try_from(text.len())?,
            byte_offset: u64::try_from(offset)?,
            text: fragment,
            end_of_field: end == text.len(),
            next_cursor,
        })
    }
}

fn validate_request(request: &HistoryWindowRequest) -> Result<()> {
    HistoryPageLimit::for_collection(request.limit.get(), &request.collection)?;
    validate_response_bound(request)?;
    if request.cursor.is_some() && request.anchor_node_id.is_some() {
        return Err(invalid("history anchor and cursor are mutually exclusive"));
    }
    if request.anchor_node_id.is_some() && request.collection != HistoryCollection::Nodes {
        return Err(invalid("history anchor requires the node collection"));
    }
    if (request.collection.requires_root() || request.anchor_node_id.is_some())
        && request.history_root.is_none()
        && request.cursor.is_none()
    {
        return Err(invalid(
            "this history read requires its source history root",
        ));
    }
    Ok(())
}

/// Encoded-size proofs, not additional record or history limits. Scalar
/// summaries contain only admitted IDs, 240-byte names, 128-byte notes,
/// 768-byte paths and at most 16 128-byte tags. JSON escaping costs at most
/// six bytes per source byte; arbitrary author/message text is external.
/// The shared node bound includes its parent cursor. Other summary bounds
/// cover their fixed keys, scalar counters and field-length descriptors.
fn validate_response_bound(request: &HistoryWindowRequest) -> Result<()> {
    let record_bytes = match request.collection {
        HistoryCollection::Nodes => MAX_HISTORY_NODE_ENCODED_BYTES,
        HistoryCollection::Versions => 8192,
        HistoryCollection::Branches | HistoryCollection::Merges => 4096,
        HistoryCollection::NodeParents { .. }
        | HistoryCollection::VersionSources { .. }
        | HistoryCollection::MergeInputs { .. }
        | HistoryCollection::MergeParents { .. } => 512,
        HistoryCollection::NodeField { .. } => 6,
    };
    let preview_bytes = if request.collection == HistoryCollection::Nodes {
        // Two strictly parsed ASCII IDs plus the endpoint token and JSON keys.
        MAX_HISTORY_PREVIEW_EDGES * 256
    } else {
        0
    };
    // The shared limit has already been admitted: every product and sum fits
    // even a 32-bit usize. Envelope covers context and both page cursors.
    let maximum = MAX_HISTORY_ENVELOPE_BYTES + request.limit.get() * record_bytes + preview_bytes;
    if maximum > MAX_HISTORY_RESPONSE_BYTES {
        return Err(Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "shared history page policy exceeds the encoded response budget",
        ));
    }
    Ok(())
}

fn validate_scope(
    state: &State,
    request: &HistoryWindowRequest,
    context: &HistoryResolvedContext,
    cancel: &HistoryWindowCancellation,
) -> Result<()> {
    let history = context.history_tree_id.as_str();
    let repository = state
        .repositories
        .get(context.repository_id.as_str())
        .filter(|repository| repository.history_tree_id == history)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNotFound,
                "history repository context is absent",
            )
        })?;
    if state
        .histories
        .get(history)
        .is_none_or(|history| history.repository_id != repository.repository_id)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNotFound,
            "history repository ownership differs",
        ));
    }
    let mut found = false;
    for version in state.versions.values() {
        cancel.checkpoint()?;
        if version.file_id == request.file_id.as_str() && version.history_tree_id == history {
            found = true;
            break;
        }
    }
    if !found {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNotFound,
            "file is outside this history",
        ));
    }
    if let Some(location) = &request.location
        && (location.repository_id != context.repository_id
            || context
                .path
                .as_ref()
                .is_none_or(|path| path.key() != location.path.key()))
    {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNotFound,
            "file is outside the requested logical path",
        ));
    }
    Ok(())
}

fn validate_cursor<'a>(
    state: &State,
    request: &'a HistoryWindowRequest,
    context: &HistoryResolvedContext,
    root: HistoryRoot,
) -> Result<(Option<&'a HistoryNodeId>, HistoryQueryHash)> {
    let cursor = request.cursor.as_ref().map(HistoryCursor::payload);
    if request
        .history_root
        .zip(cursor.map(|cursor| cursor.history_root))
        .is_some_and(|(explicit, cursor)| explicit != cursor)
    {
        return Err(invalid("explicit history root differs from cursor"));
    }
    if request
        .history_root
        .is_some_and(|expected| expected != root)
        || cursor.is_some_and(|cursor| cursor.history_root != root)
    {
        return Err(changed());
    }
    let anchor = cursor
        .and_then(|cursor| cursor.anchor_node_id.as_ref())
        .or(request.anchor_node_id.as_ref());
    if anchor.is_some_and(|anchor| {
        nodes::node(
            state,
            Scope {
                file: request.file_id.as_str(),
                history: context.history_tree_id.as_str(),
            },
            anchor.as_str(),
        )
        .is_none()
    }) {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNodeNotFound,
            "history anchor is outside this file and history",
        ));
    }
    let query_hash = request.query_hash(context, anchor);
    if let Some(cursor) = cursor {
        cursor.validate()?;
        if cursor.file_id != request.file_id
            || cursor.history_tree_id != context.history_tree_id
            || cursor.collection != request.collection
            || cursor.limit != request.limit
            || cursor.query_hash != query_hash
        {
            return Err(invalid("history cursor does not match this query"));
        }
    }
    Ok((anchor, query_hash))
}
