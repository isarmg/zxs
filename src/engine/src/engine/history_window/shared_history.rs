//! Request-local automatic components over retained nonempty content metadata.
//! Equality is separate from ancestry: revisiting bytes never folds event IDs.
use super::{HistoryCancellationCheckpoint, nodes, query};
use crate::{Error, FileVersion, Result, State};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use storage_contract::history_window::{
    HistoryBranchId, HistoryFileId, HistoryTreeId, HistoryVersionId, HistoryWindowCancellation,
};
use storage_contract::shared_history::{
    MAX_SHARED_HISTORY_BRANCHES, MAX_SHARED_HISTORY_EDGES, MAX_SHARED_HISTORY_FILES,
    MAX_SHARED_HISTORY_NODES, SharedContent, SharedHistoryBranch, SharedHistoryFile,
    SharedHistoryGraph, SharedHistoryNode, SharedHistoryNodeId, SharedHistoryNodeKind,
    SharedHistoryRequest, SharedHistoryVersion,
};

const MAX_SCAN_RECORDS: usize = 1_048_576;
const MAX_INDEX_VERSIONS: usize = 65_536;
type ContentKey = (storage_contract::ContentHash, u64);

fn exhausted() -> Error {
    Error::new(
        storage_contract::ErrorCode::ResourceLimit,
        "shared history exceeds its complete-component lookup budget",
    )
}

struct Scan<'a> {
    remaining: usize,
    cancellation: &'a HistoryWindowCancellation,
}
impl Scan<'_> {
    fn record(&mut self) -> Result<()> {
        self.cancellation.checkpoint()?;
        if self.remaining == 0 {
            return Err(exhausted());
        }
        self.remaining -= 1;
        Ok(())
    }
}

struct VersionIndex<'a> {
    files: BTreeMap<&'a str, Vec<&'a FileVersion>>,
    contents: BTreeMap<ContentKey, Vec<&'a FileVersion>>,
}
impl<'a> VersionIndex<'a> {
    fn new(state: &'a State, history: &str, scan: &mut Scan<'_>) -> Result<Self> {
        let mut index = Self {
            files: BTreeMap::new(),
            contents: BTreeMap::new(),
        };
        let mut count = 0;
        for version in state.versions.values() {
            scan.record()?;
            if version.history_tree_id != history {
                continue;
            }
            if count == MAX_INDEX_VERSIONS {
                return Err(exhausted());
            }
            count += 1;
            index
                .files
                .entry(&version.file_id)
                .or_default()
                .push(version);
            if version.logical_size != 0 {
                index
                    .contents
                    .entry(content_key(version))
                    .or_default()
                    .push(version);
            }
        }
        Ok(index)
    }

    fn component(&self, origin: &str, scan: &mut Scan<'_>) -> Result<BTreeSet<&'a str>> {
        let origin = self
            .files
            .get_key_value(origin)
            .expect("invariant: admitted file has a version")
            .0;
        let mut files = BTreeSet::from([*origin]);
        let mut queue = VecDeque::from([*origin]);
        let mut seen = BTreeSet::new();
        let mut versions = 0;
        while let Some(file) = queue.pop_front() {
            for version in &self.files[file] {
                scan.record()?;
                if versions == MAX_SHARED_HISTORY_NODES {
                    return Err(exhausted());
                }
                versions += 1;
                if version.logical_size == 0 || !seen.insert(content_key(version)) {
                    continue;
                }
                for related in &self.contents[&content_key(version)] {
                    scan.record()?;
                    if !files.contains(related.file_id.as_str()) {
                        if files.len() == MAX_SHARED_HISTORY_FILES {
                            return Err(exhausted());
                        }
                        files.insert(&related.file_id);
                        queue.push_back(&related.file_id);
                    }
                }
            }
        }
        Ok(files)
    }
}

fn content_key(version: &FileVersion) -> ContentKey {
    (version.content_hash, version.logical_size)
}

struct Events {
    nodes: BTreeMap<String, SharedHistoryNode>,
    edges: BTreeMap<String, BTreeSet<String>>,
    edge_count: usize,
}
impl Events {
    fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            edge_count: 0,
        }
    }
    fn insert(
        &mut self,
        id: &str,
        file: &str,
        kind: SharedHistoryNodeKind,
        body: Option<&str>,
        sequence: u64,
    ) -> Result<()> {
        if self.nodes.len() == MAX_SHARED_HISTORY_NODES {
            return Err(exhausted());
        }
        let node = SharedHistoryNode {
            node_id: SharedHistoryNodeId::parse(id)?,
            file_id: HistoryFileId::parse(file)?,
            kind,
            version_id: body.map(HistoryVersionId::parse).transpose()?,
            sequence,
            parent_node_ids: Vec::new(),
        };
        assert!(
            self.nodes.insert(id.into(), node).is_none(),
            "invariant: admitted event IDs are unique"
        );
        Ok(())
    }
    fn parent(&mut self, child: &str, parent: &str, scan: &mut Scan<'_>) -> Result<()> {
        scan.record()?;
        // Cross-file content source proofs are not same-file temporal edges.
        if child == parent
            || self
                .nodes
                .get(parent)
                .is_none_or(|parent| parent.file_id != self.nodes[child].file_id)
        {
            return Ok(());
        }
        let parents = self.edges.entry(child.into()).or_default();
        if !parents.contains(parent) {
            if self.edge_count == MAX_SHARED_HISTORY_EDGES {
                return Err(exhausted());
            }
            self.edge_count += 1;
            parents.insert(parent.into());
        }
        Ok(())
    }
    fn finish(mut self) -> Result<Vec<SharedHistoryNode>> {
        for (child, parents) in self.edges {
            self.nodes
                .get_mut(&child)
                .expect("invariant: parent edges name admitted events")
                .parent_node_ids = parents
                .iter()
                .map(|id| SharedHistoryNodeId::parse(id))
                .collect::<storage_contract::Result<_>>()?;
        }
        Ok(self.nodes.into_values().collect())
    }
}

pub(super) fn project(
    query: &query::Query<'_>,
    request: &SharedHistoryRequest,
) -> Result<SharedHistoryGraph> {
    let mut scan = Scan {
        remaining: MAX_SCAN_RECORDS,
        cancellation: query.cancel,
    };
    let history = query.context.history_tree_id.as_str();
    let index = VersionIndex::new(query.state, history, &mut scan)?;
    let component = index.component(request.file_id.as_str(), &mut scan)?;
    let mut graph = SharedHistoryGraph {
        origin_file_id: request.file_id.clone(),
        history_tree_id: HistoryTreeId::parse(history)?,
        history_root: request.history_root,
        files: Vec::new(),
        versions: Vec::new(),
        nodes: Vec::new(),
        branches: Vec::new(),
        shared_contents: Vec::new(),
    };
    let mut events = Events::new();
    for file in &component {
        let versions = &index.files[file];
        graph.files.push(SharedHistoryFile {
            file_id: HistoryFileId::parse(file)?,
            creation_name: versions[0].creation_name.clone(),
        });
        for version in versions {
            scan.record()?;
            graph.versions.push(SharedHistoryVersion {
                version_id: HistoryVersionId::parse(&version.version_id)?,
                file_id: HistoryFileId::parse(file)?,
                creation_name: version.creation_name.clone(),
                logical_size: version.logical_size,
                content_hash: version.content_hash,
                content_state: query.state.content_states.get(&version.version_id).copied(),
            });
            events.insert(
                &version.version_id,
                file,
                SharedHistoryNodeKind::Content,
                Some(&version.version_id),
                version.version_sequence,
            )?;
        }
    }
    metadata_events(query.state, history, &component, &mut events, &mut scan)?;
    ancestry(query.state, history, &mut events, &mut scan)?;
    graph.branches = branches(query.state, history, &component, &mut scan)?;
    graph.nodes = events.finish()?;
    graph
        .versions
        .sort_by(|a, b| a.version_id.cmp(&b.version_id));
    for ((content_hash, logical_size), versions) in index.contents {
        scan.record()?;
        if component.contains(versions[0].file_id.as_str()) {
            let mut files = BTreeSet::new();
            let mut ids = Vec::new();
            for version in versions {
                scan.record()?;
                // Component closure includes the entire equality bucket.
                files.insert(&version.file_id);
                ids.push(HistoryVersionId::parse(&version.version_id)?);
            }
            if files.len() > 1 {
                graph.shared_contents.push(SharedContent {
                    logical_size,
                    content_hash,
                    version_ids: ids,
                });
            }
        }
    }
    query.cancel.checkpoint()?;
    Ok(graph)
}

fn metadata_events(
    state: &State,
    history: &str,
    files: &BTreeSet<&str>,
    events: &mut Events,
    scan: &mut Scan<'_>,
) -> Result<()> {
    for node in state.file_path_nodes.values() {
        scan.record()?;
        if node.history_tree_id == history && files.contains(node.file_id.as_str()) {
            events.insert(
                &node.node_id,
                &node.file_id,
                SharedHistoryNodeKind::Rename,
                Some(&node.version_id),
                node.sequence,
            )?;
        }
    }
    for node in state.file_metadata_nodes.values() {
        scan.record()?;
        if node.history_tree_id == history && files.contains(node.file_id.as_str()) {
            let kind = match node.kind {
                crate::FileMetadataKind::Tags => SharedHistoryNodeKind::Tags,
                crate::FileMetadataKind::Merge => SharedHistoryNodeKind::Merge,
            };
            events.insert(
                &node.node_id,
                &node.file_id,
                kind,
                Some(&node.version_id),
                node.sequence,
            )?;
        }
    }
    for node in state.file_deletions.values() {
        scan.record()?;
        if node.history_tree_id == history && files.contains(node.file_id.as_str()) {
            events.insert(
                &node.node_id,
                &node.file_id,
                SharedHistoryNodeKind::Deletion,
                None,
                node.sequence,
            )?;
        }
    }
    Ok(())
}

fn ancestry(state: &State, history: &str, events: &mut Events, scan: &mut Scan<'_>) -> Result<()> {
    let ids = events.nodes.keys().cloned().collect::<Vec<_>>();
    for id in &ids {
        scan.record()?;
        let node = &events.nodes[id];
        let parents = if node.kind == SharedHistoryNodeKind::Deletion {
            &state.file_deletions[node.file_id.as_str()].parent_node_ids
        } else {
            nodes::base_parents(state, id)
        };
        for parent in parents {
            events.parent(id, parent, scan)?;
        }
    }
    for merge in state.file_branch_merges.values() {
        scan.record()?;
        if merge.history_tree_id == history && events.nodes.contains_key(&merge.result_node_id) {
            for parent in &merge.parent_node_ids {
                events.parent(&merge.result_node_id, parent, scan)?;
            }
        }
    }
    Ok(())
}

fn branches(
    state: &State,
    history: &str,
    files: &BTreeSet<&str>,
    scan: &mut Scan<'_>,
) -> Result<Vec<SharedHistoryBranch>> {
    let mut branches = Vec::new();
    for branch in state.branches.values() {
        scan.record()?;
        let Some(file) = branch
            .file_id
            .as_deref()
            .filter(|file| files.contains(file))
        else {
            continue;
        };
        if branch.history_tree_id != history {
            continue;
        }
        if branches.len() == MAX_SHARED_HISTORY_BRANCHES {
            return Err(exhausted());
        }
        let mut head = branch.file_node_id.as_deref();
        if head.is_none() {
            for entry in &state.commits[&branch.head_commit_id].entries {
                scan.record()?;
                if entry.kind == crate::NamespaceEntryKind::File && entry.file_id == file {
                    head = Some(&entry.version_id);
                    break;
                }
            }
        }
        let head = head.expect("invariant: admitted file branch has a retained head");
        branches.push(SharedHistoryBranch {
            branch_id: HistoryBranchId::parse(&branch.branch_id)?,
            file_id: HistoryFileId::parse(file)?,
            name: branch.name.clone(),
            head_node_id: SharedHistoryNodeId::parse(head)?,
            state: branch.state,
            merged_into: state
                .file_branch_aliases
                .get(&branch.branch_id)
                .map(|id| HistoryBranchId::parse(id))
                .transpose()?,
        });
    }
    Ok(branches)
}
