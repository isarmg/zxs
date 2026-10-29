//! Bounded page projections over one file's history in one admitted State.
//!
//! Nothing here materializes a whole history. Candidate sets come from the
//! derived owner index (`DerivedOwners`) and from the file's own branches;
//! selection keeps at most `limit + 1` keys. Records are model values, not
//! wire DTOs: the server formats timestamps and u64 text at its boundary.
use super::*;
use crate::state_projection::DerivedOwners;
use std::collections::BTreeSet;
use storage_contract::client_protocol::MAX_INLINE_TEXT_BYTES;

/// Canonical page direction. `Forward` reads after the boundary in canonical
/// display order; `Backward` reads before it. Both return keys in canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// Node order: sequence descending, then node id ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeKey {
    pub sequence: u64,
    pub node_id: String,
}
impl Ord for NodeKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .sequence
            .cmp(&self.sequence)
            .then_with(|| self.node_id.cmp(&other.node_id))
    }
}
impl PartialOrd for NodeKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Merge order: sequence descending, then merge id ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeKey {
    pub sequence: u64,
    pub merge_id: String,
}
impl Ord for MergeKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .sequence
            .cmp(&self.sequence)
            .then_with(|| self.merge_id.cmp(&other.merge_id))
    }
}
impl PartialOrd for MergeKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Body version order: version sequence ascending, then version id ascending.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionKey {
    pub version_sequence: u64,
    pub version_id: String,
}

/// Keys in canonical order, at most `limit` of them, with the two directions'
/// continuation facts computed from the candidate set, not from the page size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection<K> {
    pub keys: Vec<K>,
    pub more_before: bool,
    pub more_after: bool,
}

/// An offset slice of an index-addressed record list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub offset: usize,
}

/// Text without a persisted write bound: inline only when short.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedText {
    pub bytes: usize,
    pub inline: Option<String>,
}
impl BoundedText {
    fn new(text: &str) -> Self {
        Self {
            bytes: text.len(),
            inline: (text.len() <= MAX_INLINE_TEXT_BYTES).then(|| text.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Content,
    Rename,
    Tags,
    Merge,
}
impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Rename => "rename",
            Self::Tags => "tags",
            Self::Merge => "merge",
        }
    }
}

/// One node with every bounded field. Parents are a separate paged relation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRecord {
    pub node_id: String,
    pub kind: NodeKind,
    pub version_id: String,
    pub sequence: u64,
    /// Content nodes carry the version's RFC 3339 text; other nodes carry Unix seconds.
    pub created_at: NodeTime,
    pub branch_id: Option<String>,
    pub path: String,
    pub previous_path: Option<String>,
    pub note: String,
    pub tags: BTreeSet<String>,
    pub size: u64,
    pub content_hash: ContentHash,
    pub content_state: Option<ContentState>,
    pub download_name: String,
    pub storage_name: String,
    pub operation_id: Option<OperationId>,
    pub author: BoundedText,
    pub message: BoundedText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeTime {
    Rfc3339(String),
    UnixSeconds(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchRecord {
    pub branch_id: String,
    pub history_tree_id: String,
    pub name: String,
    pub head_commit_id: String,
    pub fork_commit_id: String,
    pub branch_kind: BranchKind,
    pub file_node_id: Option<String>,
    pub generation: u64,
    pub state: BranchState,
    /// Head version visible in the head commit's entries, if any.
    pub visible_version_id: Option<String>,
    /// Pending (inherited) source for this file, merged commit then branch.
    pub source_version_id: Option<String>,
    pub read_only: bool,
    pub alias_of: Option<String>,
    pub node_id: Option<String>,
    pub pending_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeRecord {
    pub merge_id: String,
    pub sequence: u64,
    pub chosen_node_id: String,
    pub result_node_id: String,
    pub surviving_branch_id: String,
    pub reused_node: bool,
    pub operation_id: OperationId,
    pub input_count: usize,
    pub parent_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeField {
    Message,
    Author,
}
impl NodeField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Author => "author",
        }
    }
}

/// A UTF-8 safe byte window of one text field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldSlice {
    pub total_bytes: usize,
    pub offset: usize,
    pub text: String,
    pub complete: bool,
}

/// Where the current entry of this file is looked up.
#[derive(Clone, Copy, Debug)]
pub enum ContextLocation<'a> {
    /// Deterministic owner rule: file location, else first tree entry.
    Resolve,
    /// An explicit path key in the repository's main namespace.
    PathKey(&'a str),
}

/// Bounded current-context summary of one file in one repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentContext {
    pub repository_id: String,
    pub path: Option<String>,
    pub note: String,
    pub current_node_id: Option<String>,
    pub selected_branch_id: Option<String>,
    /// The current entry's version with its inherited source, when present.
    pub current: Option<CurrentEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentEntry {
    pub version_id: String,
    pub source_version_id: Option<String>,
    pub name: String,
}

/// Borrowed read scope: one file, one history, one admitted State. Obtain it
/// from [`Engine::file_history_scope`]; it borrows the engine and grants no
/// authorization.
#[derive(Clone, Copy)]
pub struct FileHistoryScope<'a> {
    pub(crate) engine: &'a Engine,
    pub(crate) state: &'a State,
    pub(crate) owners: &'a DerivedOwners,
    pub(crate) file_id: &'a str,
    pub(crate) history_id: &'a str,
}

impl Engine {
    /// A bounded read scope over one file's history in the current State.
    ///
    /// Warm engines borrow the publication's owner index; a cold State builds
    /// it once in O(State) and keeps it with the catalog bound to that State.
    ///
    /// # Errors
    /// `RecoveryRequired` when no admitted history catalog binds the current State.
    pub fn file_history_scope<'a>(
        &'a self,
        file_id: &'a str,
        history_id: &'a str,
    ) -> Result<FileHistoryScope<'a>> {
        Ok(FileHistoryScope {
            engine: self,
            state: &self.state,
            owners: self.history_owners()?,
            file_id,
            history_id,
        })
    }
}

impl<'a> FileHistoryScope<'a> {
    pub fn file_id(&self) -> &'a str {
        self.file_id
    }
    pub fn history_id(&self) -> &'a str {
        self.history_id
    }
    /// The admitted State this scope reads; borrowed from the engine.
    pub fn state(&self) -> &'a State {
        self.state
    }
}

fn node_not_found() -> Error {
    Error::new(
        storage_contract::ErrorCode::FileNodeNotFound,
        "node does not belong to this file history",
    )
}

fn merge_not_found() -> Error {
    Error::new(
        storage_contract::ErrorCode::NotFound,
        "merge does not belong to this file history",
    )
}

fn version_not_found() -> Error {
    Error::new(
        storage_contract::ErrorCode::VersionNotFound,
        "version does not belong to this file history",
    )
}

fn branch_not_found() -> Error {
    Error::new(
        storage_contract::ErrorCode::FileBranchNotFound,
        "branch does not belong to this file history",
    )
}

/// Retains at most `limit + 1` keys around a boundary; cardinality independent.
struct Selector<K> {
    boundary: Option<(K, Direction)>,
    limit: usize,
    candidates: BTreeSet<K>,
    before: bool,
    after: bool,
}
impl<K: Ord + Clone> Selector<K> {
    fn new(boundary: Option<(&K, Direction)>, limit: usize) -> Self {
        Self {
            boundary: boundary.map(|(key, direction)| (key.clone(), direction)),
            limit,
            candidates: BTreeSet::new(),
            before: false,
            after: false,
        }
    }
    fn backward(&self) -> bool {
        matches!(self.boundary, Some((_, Direction::Backward)))
    }
    fn consider(&mut self, key: K) {
        if let Some((boundary, direction)) = &self.boundary {
            match direction {
                Direction::Backward if key >= *boundary => {
                    self.after = true;
                    return;
                }
                Direction::Forward if key <= *boundary => {
                    self.before = true;
                    return;
                }
                Direction::Forward | Direction::Backward => {}
            }
        }
        self.candidates.insert(key);
        if self.candidates.len() > self.limit + 1 {
            if self.backward() {
                self.candidates.pop_first();
            } else {
                self.candidates.pop_last();
            }
        }
    }
    fn finish(mut self) -> Selection<K> {
        if self.candidates.len() > self.limit {
            if self.backward() {
                self.candidates.pop_first();
                self.before = true;
            } else {
                self.candidates.pop_last();
                self.after = true;
            }
        }
        Selection {
            keys: self.candidates.into_iter().collect(),
            more_before: self.before,
            more_after: self.after,
        }
    }
}

fn slice<T>(total: usize, offset: usize, limit: usize, item: impl Fn(usize) -> T) -> Slice<T> {
    let end = offset.saturating_add(limit).min(total);
    let items = (offset.min(total)..end).map(item).collect();
    Slice {
        items,
        total,
        offset,
    }
}

impl<'a> FileHistoryScope<'a> {
    fn version(&self, id: &str) -> Option<&'a FileVersion> {
        self.state.versions.get(id).filter(|version| {
            version.file_id == self.file_id && version.history_tree_id == self.history_id
        })
    }
    fn path_node(&self, id: &str) -> Option<&'a FilePathNode> {
        self.state
            .file_path_nodes
            .get(id)
            .filter(|node| node.file_id == self.file_id && node.history_tree_id == self.history_id)
    }
    fn metadata_node(&self, id: &str) -> Option<&'a FileMetadataNode> {
        self.state
            .file_metadata_nodes
            .get(id)
            .filter(|node| node.file_id == self.file_id && node.history_tree_id == self.history_id)
    }
    fn merge(&self, id: &str) -> Option<&'a FileBranchMerge> {
        self.state.file_branch_merges.get(id).filter(|merge| {
            merge.file_id == self.file_id && merge.history_tree_id == self.history_id
        })
    }
    fn branch(&self, id: &str) -> Option<&'a Branch> {
        self.state.branches.get(id).filter(|branch| {
            branch.file_id.as_deref() == Some(self.file_id)
                && branch.history_tree_id == self.history_id
                && branch.state != BranchState::Migrated
        })
    }
    /// Whether an id names a node of this file history (any kind).
    fn contains_node(&self, id: &str) -> bool {
        self.version(id).is_some()
            || self.path_node(id).is_some()
            || self.metadata_node(id).is_some()
    }
    fn version_ids(&self) -> impl Iterator<Item = &'a String> + '_ {
        self.owners
            .versions(self.file_id)
            .filter(move |id| self.version(id).is_some())
    }
    fn file_rows(&self) -> Option<&'a crate::state_projection::FileRows> {
        self.owners.file_rows(self.file_id)
    }
    fn merges(&self) -> impl Iterator<Item = &'a FileBranchMerge> + '_ {
        self.file_rows()
            .into_iter()
            .flat_map(|rows| rows.merges.keys())
            .filter_map(move |id| self.merge(id))
    }
    fn content_sequence(&self, version: &FileVersion) -> u64 {
        self.owners
            .first_commit(&version.version_id)
            .map_or(version.version_sequence, |first| first.sequence)
    }
    fn first_commit(&self, version_id: &str) -> Option<&'a Commit> {
        self.owners
            .first_commit(version_id)
            .and_then(|first| self.state.commits.get(&first.commit_id))
    }
    fn commit_entry(&self, commit: &Commit, version_id: &str) -> Result<Option<Entry>> {
        let tree = self
            .engine
            .commit_namespace(&commit.commit_id)
            .expect("invariant: first publisher is an admitted commit");
        let mut found = None;
        tree.walk(|entry| {
            if entry.file_id == self.file_id && entry.version_id == version_id {
                found = Some(entry.clone());
                return Ok(false);
            }
            Ok(true)
        })?;
        Ok(found)
    }

    fn head_branch(&self, version_id: &str) -> Result<Option<String>> {
        for (id, branch) in self
            .state
            .branches
            .iter()
            .filter(|(_, branch)| branch.file_id.as_deref() == Some(self.file_id))
        {
            let commit = self
                .state
                .commits
                .get(&branch.head_commit_id)
                .expect("invariant: admitted branch head exists");
            if !branch.pending_file_versions.contains_key(self.file_id)
                && !commit.pending_file_versions.contains_key(self.file_id)
                && self.commit_entry(commit, version_id)?.is_some()
            {
                return Ok(Some(id.clone()));
            }
        }
        Ok(None)
    }

    fn base_parents(&self, node: &str) -> &'a [String] {
        if let Some(node) = self.state.file_metadata_nodes.get(node) {
            &node.parent_node_ids
        } else if let Some(node) = self.state.file_path_nodes.get(node) {
            &node.parent_node_ids
        } else if let Some(version) = self.state.versions.get(node) {
            self.state
                .file_node_parents
                .get(node)
                .unwrap_or(&version.source_version_ids)
        } else {
            &[]
        }
    }

    fn parent_ids<'b>(&'b self, node: &'b str) -> impl Iterator<Item = &'a String> + 'b {
        self.base_parents(node).iter().chain(
            self.merges()
                .filter(move |event| event.result_node_id == node)
                .flat_map(|event| {
                    event
                        .parent_node_ids
                        .iter()
                        .filter(move |parent| parent.as_str() != node)
                }),
        )
    }

    /// Admitted histories are acyclic. Follow only the inherited first-parent
    /// chain, retaining no ancestor frontier/cache and copying one tag set.
    fn tags(&self, node: &str) -> BTreeSet<String> {
        let mut current = node;
        loop {
            if let Some(metadata) = self.state.file_metadata_nodes.get(current) {
                return metadata.tags.clone();
            }
            match self.base_parents(current).first() {
                Some(parent) => current = parent,
                None => return BTreeSet::new(),
            }
        }
    }

    /// The canonical key of one node, or `FileNodeNotFound`.
    ///
    /// # Errors
    /// `FileNodeNotFound` when the id is not a node of this file history.
    pub fn node_key(&self, node_id: &str) -> Result<NodeKey> {
        let sequence = if let Some(version) = self.version(node_id) {
            self.content_sequence(version)
        } else if let Some(node) = self.path_node(node_id) {
            node.sequence
        } else if let Some(node) = self.metadata_node(node_id) {
            node.sequence
        } else {
            return Err(node_not_found());
        };
        Ok(NodeKey {
            sequence,
            node_id: node_id.to_owned(),
        })
    }

    pub fn select_nodes(
        &self,
        boundary: Option<(&NodeKey, Direction)>,
        limit: usize,
    ) -> Selection<NodeKey> {
        let mut selector = Selector::new(boundary, limit);
        for id in self.version_ids() {
            let version = self
                .version(id)
                .expect("invariant: version ids were filtered to this scope");
            selector.consider(NodeKey {
                sequence: self.content_sequence(version),
                node_id: id.clone(),
            });
        }
        if let Some(rows) = self.file_rows() {
            for id in rows.path_nodes.keys() {
                if let Some(node) = self.path_node(id) {
                    selector.consider(NodeKey {
                        sequence: node.sequence,
                        node_id: id.clone(),
                    });
                }
            }
            for id in rows.metadata_nodes.keys() {
                if let Some(node) = self.metadata_node(id) {
                    selector.consider(NodeKey {
                        sequence: node.sequence,
                        node_id: id.clone(),
                    });
                }
            }
        }
        selector.finish()
    }

    /// Constant-space reader. Only selected nodes are projected; source
    /// relations are borrowed and scanned rather than cached per request.
    pub fn node_reader(&self) -> NodeReader<'a> {
        NodeReader { scope: *self }
    }

    /// One node with all bounded fields through a fresh reader.
    ///
    /// # Errors
    /// `FileNodeNotFound` when the key is not a node of this file history.
    pub fn node_record(&self, key: &NodeKey) -> Result<NodeRecord> {
        self.node_reader().record(key)
    }

    fn body(&self, version_id: &str) -> &'a FileVersion {
        self.state
            .versions
            .get(version_id)
            .expect("invariant: admitted file nodes reference an existing body")
    }
    fn download_name(&self, creation_name: &str) -> String {
        self.state
            .file_names
            .get(self.file_id)
            .cloned()
            .unwrap_or_else(|| creation_name.to_owned())
    }
    fn version_note(&self, version_id: &str) -> String {
        self.state
            .version_notes
            .get(version_id)
            .cloned()
            .unwrap_or_default()
    }
    fn node_note(&self, node_id: &str) -> String {
        self.state
            .file_path_notes
            .get(node_id)
            .cloned()
            .unwrap_or_default()
    }
    /// Resolve a metadata node's path through its source chain: merge nodes
    /// follow the chosen node, other nodes their first parent. Bounded by the
    /// chain length; admitted chains are acyclic.
    fn metadata_path(&self, node: &FileMetadataNode) -> Result<String> {
        let mut current = node.node_id.as_str();
        loop {
            if let Some(path) = self.state.file_path_nodes.get(current) {
                return Ok(path.path.clone());
            }
            if let Some(version) = self.state.versions.get(current) {
                let entry = self
                    .first_commit(current)
                    .map(|commit| self.commit_entry(commit, current))
                    .transpose()?
                    .flatten();
                return Ok(entry.map_or_else(
                    || version.creation_name.clone(),
                    |entry| entry.path().to_owned(),
                ));
            }
            let metadata = self
                .state
                .file_metadata_nodes
                .get(current)
                .expect("invariant: admitted file history references an existing node");
            let source = if metadata.kind == FileMetadataKind::Merge {
                self.merges()
                    .filter(|merge| !merge.reused_node && merge.result_node_id == current)
                    .last()
                    .map(|merge| merge.chosen_node_id.as_str())
            } else {
                metadata.parent_node_ids.first().map(String::as_str)
            };
            match source {
                Some(source) => current = source,
                None => return Ok(self.body(&metadata.version_id).creation_name.clone()),
            }
        }
    }

    /// Parents of one node within this file history, id ascending, after an
    /// exclusive lower bound, through a fresh reader.
    ///
    /// # Errors
    /// `FileNodeNotFound` when the node is not in this file history.
    pub fn node_parents(
        &self,
        node_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Selection<String>> {
        self.node_reader().parents(node_id, after, limit)
    }

    /// Both directions use the same canonical parent relation. Only the
    /// bounded selector owns keys; source arrays are never cloned wholesale.
    pub fn node_parent_window(
        &self,
        node_id: &str,
        boundary: Option<(&str, Direction)>,
        limit: usize,
    ) -> Result<Selection<String>> {
        if !self.contains_node(node_id) {
            return Err(node_not_found());
        }
        if let Some((parent, _)) = boundary
            && !self
                .parent_ids(node_id)
                .any(|id| id == parent && self.contains_node(id))
        {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidCursor,
                "parent cursor boundary is not in this node's parent relation",
            ));
        }
        let owned = boundary.map(|(key, direction)| (key.to_owned(), direction));
        let mut selector = Selector::new(
            owned.as_ref().map(|(key, direction)| (key, *direction)),
            limit,
        );
        for parent in self.parent_ids(node_id).filter(|id| self.contains_node(id)) {
            selector.consider(parent.clone());
        }
        Ok(selector.finish())
    }

    /// Displayable branches of this file history, branch id ascending.
    pub fn select_branches(&self, after: Option<&str>, limit: usize) -> Selection<String> {
        self.branch_window(after.map(|key| (key, Direction::Forward)), limit)
    }

    pub fn branch_window(
        &self,
        boundary: Option<(&str, Direction)>,
        limit: usize,
    ) -> Selection<String> {
        let boundary = boundary.map(|(key, direction)| (key.to_owned(), direction));
        let mut selector = Selector::new(
            boundary.as_ref().map(|(key, direction)| (key, *direction)),
            limit,
        );
        for (id, _) in self
            .state
            .branches
            .iter()
            .filter(|(id, _)| self.branch(id).is_some())
        {
            selector.consider(id.clone());
        }
        selector.finish()
    }

    /// One branch without its pending map.
    ///
    /// # Errors
    /// `FileBranchNotFound` when the branch is not a displayable branch of this file history.
    pub fn branch_record(&self, branch_id: &str) -> Result<BranchRecord> {
        let branch = self.branch(branch_id).ok_or_else(branch_not_found)?;
        let head = self
            .state
            .commits
            .get(&branch.head_commit_id)
            .expect("invariant: admitted branch head references an existing commit");
        let visible = self
            .engine
            .commit_namespace(&head.commit_id)
            .expect("invariant: admitted branch head exists")
            .find_file(self.file_id)?
            .map(|entry| entry.version_id);
        let source = branch
            .pending_file_versions
            .get(self.file_id)
            .or_else(|| head.pending_file_versions.get(self.file_id))
            .cloned();
        let pending_count = head.pending_file_versions.len()
            + branch
                .pending_file_versions
                .keys()
                .filter(|file| !head.pending_file_versions.contains_key(*file))
                .count();
        let alias_of = self.state.file_branch_aliases.get(branch_id).cloned();
        Ok(BranchRecord {
            branch_id: branch.branch_id.clone(),
            history_tree_id: branch.history_tree_id.clone(),
            name: branch.name.clone(),
            head_commit_id: branch.head_commit_id.clone(),
            fork_commit_id: branch.fork_commit_id.clone(),
            branch_kind: branch.branch_kind,
            file_node_id: branch.file_node_id.clone(),
            generation: branch.generation,
            state: branch.state,
            node_id: branch.file_node_id.clone().or_else(|| visible.clone()),
            visible_version_id: visible,
            source_version_id: source,
            read_only: branch.state != BranchState::Active || alias_of.is_some(),
            alias_of,
            pending_count,
        })
    }

    pub fn select_merges(
        &self,
        boundary: Option<(&MergeKey, Direction)>,
        limit: usize,
    ) -> Selection<MergeKey> {
        let mut selector = Selector::new(boundary, limit);
        for merge in self.merges() {
            selector.consider(MergeKey {
                sequence: merge.sequence,
                merge_id: merge.merge_id.clone(),
            });
        }
        selector.finish()
    }

    /// # Errors
    /// `NotFound` when the merge is not in this file history.
    pub fn merge_record(&self, key: &MergeKey) -> Result<MergeRecord> {
        let merge = self.merge(&key.merge_id).ok_or_else(merge_not_found)?;
        if merge.sequence != key.sequence {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidCursor,
                "merge cursor sequence does not match its boundary",
            ));
        }
        Ok(MergeRecord {
            merge_id: merge.merge_id.clone(),
            sequence: merge.sequence,
            chosen_node_id: merge.chosen_node_id.clone(),
            result_node_id: merge.result_node_id.clone(),
            surviving_branch_id: merge.surviving_branch_id.clone(),
            reused_node: merge.reused_node,
            operation_id: merge.operation_id,
            input_count: merge.input_branch_ids.len(),
            parent_count: merge.parent_node_ids.len(),
        })
    }

    /// `(branch, node)` input pairs by original index; pairs are never split.
    ///
    /// # Errors
    /// `NotFound` when the merge is not in this file history.
    pub fn merge_inputs(
        &self,
        merge_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Slice<(String, String)>> {
        let merge = self.merge(merge_id).ok_or_else(merge_not_found)?;
        assert_eq!(
            merge.input_branch_ids.len(),
            merge.input_node_ids.len(),
            "invariant: admitted merge inputs are parallel arrays"
        );
        Ok(slice(
            merge.input_branch_ids.len(),
            offset,
            limit,
            |index| {
                (
                    merge.input_branch_ids[index].clone(),
                    merge.input_node_ids[index].clone(),
                )
            },
        ))
    }

    /// The merge's stored parent ids by index (distinct from graph parents).
    ///
    /// # Errors
    /// `NotFound` when the merge is not in this file history.
    pub fn merge_parents(
        &self,
        merge_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Slice<String>> {
        let merge = self.merge(merge_id).ok_or_else(merge_not_found)?;
        Ok(slice(merge.parent_node_ids.len(), offset, limit, |index| {
            merge.parent_node_ids[index].clone()
        }))
    }

    pub fn select_versions(
        &self,
        boundary: Option<(&VersionKey, Direction)>,
        limit: usize,
    ) -> Selection<VersionKey> {
        let mut selector = Selector::new(boundary, limit);
        for id in self.version_ids() {
            let version = self
                .version(id)
                .expect("invariant: version ids were filtered to this scope");
            selector.consider(VersionKey {
                version_sequence: version.version_sequence,
                version_id: id.clone(),
            });
        }
        selector.finish()
    }

    /// # Errors
    /// `VersionNotFound` when the version is not in this file history.
    pub fn version_record(&self, key: &VersionKey) -> Result<&'a FileVersion> {
        let version = self
            .version(&key.version_id)
            .ok_or_else(version_not_found)?;
        if version.version_sequence != key.version_sequence {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidCursor,
                "version cursor sequence does not match its boundary",
            ));
        }
        Ok(version)
    }

    /// Raw `source_version_ids` by index: true lineage, independent of graph parents.
    ///
    /// # Errors
    /// `VersionNotFound` when the version is not in this file history.
    pub fn version_sources(
        &self,
        version_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Slice<String>> {
        let version = self.version(version_id).ok_or_else(version_not_found)?;
        Ok(slice(
            version.source_version_ids.len(),
            offset,
            limit,
            |index| version.source_version_ids[index].clone(),
        ))
    }

    /// A UTF-8 safe window of one text field starting at a byte offset. The
    /// window ends on a char boundary at or before `offset + max_bytes`; an
    /// offset inside a multibyte char is rejected. A successful non-final
    /// fragment always advances, without exceeding the requested byte bound.
    ///
    /// # Errors
    /// `FileNodeNotFound` when the node is not in this file history;
    /// `InvalidRequest` when `offset` exceeds the field length.
    pub fn node_field(
        &self,
        node_id: &str,
        field: NodeField,
        offset: usize,
        max_bytes: usize,
    ) -> Result<FieldSlice> {
        let text: &str = if let Some(_version) = self.version(node_id) {
            let commit = self.first_commit(node_id);
            match field {
                NodeField::Message => commit.map_or("", |commit| commit.message.as_str()),
                NodeField::Author => commit.map_or("", |commit| commit.author.as_str()),
            }
        } else if let Some(node) = self.path_node(node_id) {
            match field {
                NodeField::Message => "",
                NodeField::Author => node.author.as_str(),
            }
        } else if let Some(node) = self.metadata_node(node_id) {
            match field {
                NodeField::Message => "",
                NodeField::Author => node.author.as_str(),
            }
        } else {
            return Err(node_not_found());
        };
        if offset > text.len() {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "field offset exceeds the field length",
            ));
        }
        if !text.is_char_boundary(offset) {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "field offset is inside a UTF-8 scalar",
            ));
        }
        let start = offset;
        let end = floor_char_boundary(text, start.saturating_add(max_bytes).min(text.len()));
        if end == start && start < text.len() {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "max_bytes cannot hold the next UTF-8 scalar",
            ));
        }
        Ok(FieldSlice {
            total_bytes: text.len(),
            offset: start,
            text: text[start..end].to_owned(),
            complete: end == text.len(),
        })
    }

    /// Bounded current-context summary: where the file is in its repository's
    /// main namespace, its current node and selected branch.
    ///
    /// # Errors
    /// `HistoryNotFound`/`NotFound` when the history has no repository;
    /// `BranchNotFound` when the main branch is not readable.
    pub fn current_context(&self, location: ContextLocation<'_>) -> Result<CurrentContext> {
        let history = self.state.histories.get(self.history_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::HistoryNotFound,
                "history not found",
            )
        })?;
        let repo = self
            .state
            .repositories
            .get(&history.repository_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::NotFound,
                    "repository not found",
                )
            })?;
        let main = self
            .state
            .branches
            .get(&repo.main_branch_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::BranchNotFound,
                    "branch not found",
                )
            })?;
        let commit = self
            .state
            .commits
            .get(&main.head_commit_id)
            .expect("invariant: admitted branch head references an existing commit");
        let tree = self
            .engine
            .commit_namespace(&commit.commit_id)
            .expect("invariant: admitted main branch head exists");
        let current = match location {
            ContextLocation::PathKey(key) => tree.get_key(key)?.filter(|entry| {
                entry.kind == NamespaceEntryKind::File && entry.file_id == self.file_id
            }),
            ContextLocation::Resolve => {
                let hinted = match self.state.file_locations.get(self.file_id) {
                    Some(path) => tree
                        .get(path)?
                        .filter(|entry| entry.file_id == self.file_id),
                    None => None,
                };
                match hinted {
                    Some(entry) => Some(entry),
                    None => tree.find_file(self.file_id)?,
                }
            }
        };
        let context = match current.as_ref() {
            Some(entry) => self
                .engine
                .file_context_for_current_entry(&repo.repository_id, entry),
            None => None,
        };
        let current_node = context
            .as_ref()
            .and_then(|context| context.node_id.clone())
            .or_else(|| current.as_ref().map(|entry| entry.version_id.clone()));
        let selected = context
            .as_ref()
            .map(|context| context.branch_id.clone())
            .or_else(|| self.state.file_selections.get(self.file_id).cloned());
        Ok(CurrentContext {
            repository_id: repo.repository_id.clone(),
            path: current.as_ref().map(|entry| entry.path().to_owned()),
            note: self
                .state
                .file_notes
                .get(self.file_id)
                .cloned()
                .unwrap_or_default(),
            current_node_id: current_node,
            selected_branch_id: selected,
            current: current.as_ref().map(|entry| CurrentEntry {
                version_id: entry.version_id.clone(),
                source_version_id: self.inherited_source(main, commit, entry, context.as_deref()),
                name: entry.name().to_owned(),
            }),
        })
    }

    /// The pending source of the current entry under the main branch head,
    /// mirroring the main-namespace resolution used by directory listings.
    fn inherited_source(
        &self,
        main: &Branch,
        commit: &Commit,
        entry: &Entry,
        context: Option<&FileContext>,
    ) -> Option<String> {
        if let Some(context) = context {
            if context.inherited_from.is_some() {
                return Some(entry.version_id.clone());
            }
            return self
                .state
                .branches
                .get(&context.branch_id)?
                .pending_file_versions
                .get(&entry.file_id)
                .cloned();
        }
        commit
            .pending_file_versions
            .get(&entry.file_id)
            .filter(|version| **version == entry.version_id)
            .cloned()
            .or_else(|| {
                super::file::pending_file_source(self.state, &main.branch_id, &entry.file_id)
                    .map(str::to_owned)
            })
    }
}

/// Page-local node reader; see [`FileHistoryScope::node_reader`].
pub struct NodeReader<'a> {
    scope: FileHistoryScope<'a>,
}

impl<'a> NodeReader<'a> {
    /// One node with all bounded fields; tags follow the inherited first-parent
    /// chain over this file's own merge events (memoized in this reader).
    ///
    /// # Errors
    /// `FileNodeNotFound` when the key is not a node of this file history.
    pub fn record(&mut self, key: &NodeKey) -> Result<NodeRecord> {
        let scope = self.scope;
        let id = key.node_id.as_str();
        if let Some(version) = scope.version(id) {
            let commit = scope.first_commit(id);
            let entry = commit
                .map(|commit| scope.commit_entry(commit, id))
                .transpose()?
                .flatten();
            let download_name = scope.download_name(&version.creation_name);
            return Ok(NodeRecord {
                node_id: id.to_owned(),
                kind: NodeKind::Content,
                version_id: id.to_owned(),
                sequence: scope.content_sequence(version),
                created_at: NodeTime::Rfc3339(version.created_at.clone()),
                branch_id: scope.head_branch(id)?,
                path: entry.map_or_else(|| download_name.clone(), |entry| entry.path().to_owned()),
                previous_path: None,
                note: scope.version_note(id),
                tags: scope.tags(id),
                size: version.logical_size,
                content_hash: version.content_hash,
                content_state: scope.state.content_states.get(id).copied(),
                download_name,
                storage_name: version.storage_name.clone(),
                operation_id: None,
                author: BoundedText::new(commit.map_or("", |commit| commit.author.as_str())),
                message: BoundedText::new(commit.map_or("", |commit| commit.message.as_str())),
            });
        }
        if let Some(node) = scope.path_node(id) {
            let body = scope.body(&node.version_id);
            return Ok(NodeRecord {
                node_id: id.to_owned(),
                kind: NodeKind::Rename,
                version_id: node.version_id.clone(),
                sequence: node.sequence,
                created_at: NodeTime::UnixSeconds(node.created_at),
                branch_id: Some(node.branch_id.clone()),
                path: node.path.clone(),
                previous_path: Some(node.previous_path.clone()),
                note: scope.node_note(id),
                tags: scope.tags(id),
                size: body.logical_size,
                content_hash: body.content_hash,
                content_state: scope.state.content_states.get(&node.version_id).copied(),
                download_name: scope.download_name(&body.creation_name),
                storage_name: body.storage_name.clone(),
                operation_id: Some(node.operation_id),
                author: BoundedText::new(&node.author),
                message: BoundedText::new(""),
            });
        }
        if let Some(node) = scope.metadata_node(id) {
            let body = scope.body(&node.version_id);
            return Ok(NodeRecord {
                node_id: id.to_owned(),
                kind: match node.kind {
                    FileMetadataKind::Tags => NodeKind::Tags,
                    FileMetadataKind::Merge => NodeKind::Merge,
                },
                version_id: node.version_id.clone(),
                sequence: node.sequence,
                created_at: NodeTime::UnixSeconds(node.created_at),
                branch_id: Some(node.branch_id.clone()),
                path: scope.metadata_path(node)?,
                previous_path: None,
                note: scope.node_note(id),
                tags: node.tags.clone(),
                size: body.logical_size,
                content_hash: body.content_hash,
                content_state: scope.state.content_states.get(&node.version_id).copied(),
                download_name: scope.download_name(&body.creation_name),
                storage_name: body.storage_name.clone(),
                operation_id: Some(node.operation_id),
                author: BoundedText::new(&node.author),
                message: BoundedText::new(""),
            });
        }
        Err(node_not_found())
    }

    /// Parents of one node within this file history, id ascending, after an
    /// exclusive lower bound.
    ///
    /// # Errors
    /// `FileNodeNotFound` when the node is not in this file history.
    pub fn parents(
        &self,
        node_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Selection<String>> {
        self.scope
            .node_parent_window(node_id, after.map(|key| (key, Direction::Forward)), limit)
    }
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_history_pages_tests.rs"]
mod tests;
