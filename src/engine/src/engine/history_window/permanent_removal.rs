//! Conditional topology analysis, not a retirement authority or executable plan.
//! A body and its event records are coupled; ordinary PURGED/D facts never select them.
use super::{HistoryCancellationCheckpoint, query};
use crate::{Error, ErrorCode, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use storage_contract::history_window::{
    HistoryBranchId, HistoryFileId, HistoryRoot, HistoryTreeId, HistoryVersionId,
};
use storage_contract::shared_history::{
    MAX_SHARED_HISTORY_EDGES, MAX_SHARED_HISTORY_NODES, SharedHistoryGraph, SharedHistoryNodeId,
};

const MAX_ANALYSIS_WORK: usize = 1_048_576;

/// Explicit content-event selection. Metadata/D IDs are not body identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermanentNodeRemovalSelection(Vec<HistoryVersionId>);
impl PermanentNodeRemovalSelection {
    pub fn new(mut versions: Vec<HistoryVersionId>) -> Result<Self> {
        if versions.is_empty() || versions.len() > MAX_SHARED_HISTORY_NODES {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "permanent node selection is empty or too large",
            ));
        }
        versions.sort();
        if versions.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "permanent node selection contains duplicates",
            ));
        }
        Ok(Self(versions))
    }
    pub fn versions(&self) -> &[HistoryVersionId] {
        &self.0
    }
}

/// Records which would have to leave the live graph together with this body.
/// This includes path/tag/merge events reusing the body, never another file's equal bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoupledNodeRetirement {
    version_id: HistoryVersionId,
    file_id: HistoryFileId,
    logical_size: u64,
    content_state: Option<storage_contract::ContentState>,
    event_node_ids: Vec<SharedHistoryNodeId>,
}
impl CoupledNodeRetirement {
    pub fn version_id(&self) -> &HistoryVersionId {
        &self.version_id
    }
    pub fn file_id(&self) -> &HistoryFileId {
        &self.file_id
    }
    pub fn logical_size(&self) -> u64 {
        self.logical_size
    }
    pub fn content_state(&self) -> Option<storage_contract::ContentState> {
        self.content_state
    }
    pub fn event_node_ids(&self) -> &[SharedHistoryNodeId] {
        &self.event_node_ids
    }
}

/// Hypothetical display ancestry through retired events. Not permission to
/// rewrite immutable sources, commit entries or an existing D receipt frontier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalParentProjection {
    node_id: SharedHistoryNodeId,
    parent_node_ids: Vec<SharedHistoryNodeId>,
}
impl RemovalParentProjection {
    pub fn node_id(&self) -> &SharedHistoryNodeId {
        &self.node_id
    }
    pub fn parent_node_ids(&self) -> &[SharedHistoryNodeId] {
        &self.parent_node_ids
    }
}

/// A bounded observation, deliberately not serializable and without PlanId,
/// operation receipt, write set, content lease or execute capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermanentNodeRemovalAnalysis {
    origin_file_id: HistoryFileId,
    history_tree_id: HistoryTreeId,
    history_root: HistoryRoot,
    retirements: Vec<CoupledNodeRetirement>,
    file_components: Vec<Vec<HistoryFileId>>,
    parent_projection: Vec<RemovalParentProjection>,
    retained_commit_entries: usize,
    affected_branch_heads: Vec<HistoryBranchId>,
    archived_branch_heads: Vec<HistoryBranchId>,
    frozen_files: Vec<HistoryFileId>,
    last_live_files: Vec<HistoryFileId>,
}
impl PermanentNodeRemovalAnalysis {
    pub fn origin_file_id(&self) -> &HistoryFileId {
        &self.origin_file_id
    }
    pub fn history_tree_id(&self) -> &HistoryTreeId {
        &self.history_tree_id
    }
    pub fn history_root(&self) -> HistoryRoot {
        self.history_root
    }
    pub fn retirements(&self) -> &[CoupledNodeRetirement] {
        &self.retirements
    }
    /// Each original stable file occurs exactly once, even if no body would remain.
    /// One file's disconnected event roots are never turned into separate files.
    pub fn file_components(&self) -> &[Vec<HistoryFileId>] {
        &self.file_components
    }
    pub fn would_separate_files(&self) -> bool {
        self.file_components.len() > 1
    }
    pub fn parent_projection(&self) -> &[RemovalParentProjection] {
        &self.parent_projection
    }
    /// This is a specific known dependency count, not an exhaustive execution admission.
    pub fn retained_commit_entries(&self) -> usize {
        self.retained_commit_entries
    }
    pub fn affected_branch_heads(&self) -> &[HistoryBranchId] {
        &self.affected_branch_heads
    }
    /// Conditional terminal frontiers only. Execution still checks current
    /// ownership, root, read leases, workspaces and the complete write set.
    pub fn archived_branch_heads(&self) -> &[HistoryBranchId] {
        &self.archived_branch_heads
    }
    pub fn frozen_files(&self) -> &[HistoryFileId] {
        &self.frozen_files
    }
    /// Conditional stable identities with no live body after this selection.
    /// Existing HEAD/D/context references can still prevent their termination.
    pub fn last_live_files(&self) -> &[HistoryFileId] {
        &self.last_live_files
    }
}

struct Work<'a> {
    remaining: usize,
    cancel: &'a storage_contract::history_window::HistoryWindowCancellation,
}
impl Work<'_> {
    fn step(&mut self) -> Result<()> {
        self.cancel.checkpoint()?;
        if self.remaining == 0 {
            return Err(limit());
        }
        self.remaining -= 1;
        Ok(())
    }
}
fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "permanent node analysis exceeds its complete-result budget",
    )
}

pub(super) fn analyze(
    query: &query::Query<'_>,
    graph: &SharedHistoryGraph,
    selection: &PermanentNodeRemovalSelection,
) -> Result<PermanentNodeRemovalAnalysis> {
    let mut work = Work {
        remaining: MAX_ANALYSIS_WORK,
        cancel: query.cancel,
    };
    let versions = graph
        .versions
        .iter()
        .map(|v| (&v.version_id, v))
        .collect::<BTreeMap<_, _>>();
    let mut retirements = Vec::new();
    let mut retired = BTreeSet::new();
    let selected = selection.0.iter().collect::<BTreeSet<_>>();
    let mut selected_files = BTreeSet::new();
    for id in &selection.0 {
        work.step()?;
        let version = versions.get(id).ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "selected version is outside this complete shared component",
            )
        })?;
        selected_files.insert(version.file_id.clone());
        retirements.push(CoupledNodeRetirement {
            version_id: id.clone(),
            file_id: version.file_id.clone(),
            logical_size: version.logical_size,
            content_state: version.content_state,
            event_node_ids: Vec::new(),
        });
    }
    let indices = selection
        .0
        .iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect::<BTreeMap<_, _>>();
    for node in &graph.nodes {
        work.step()?;
        if let Some(index) = node.version_id.as_ref().and_then(|id| indices.get(id)) {
            retired.insert(node.node_id.clone());
            retirements[*index]
                .event_node_ids
                .push(node.node_id.clone());
        }
    }
    let file_components = components(graph, &selected, &mut work)?;
    let parent_projection = parents(graph, &retired, &mut work)?;
    let selected_text = selection
        .0
        .iter()
        .map(HistoryVersionId::as_str)
        .collect::<BTreeSet<_>>();
    let mut retained_commit_entries = 0;
    for commit in query.state.commits.values() {
        work.step()?;
        if commit.history_tree_id != graph.history_tree_id.as_str() {
            continue;
        }
        for entry in &commit.entries {
            work.step()?;
            if selected_text.contains(entry.version_id.as_str()) {
                retained_commit_entries += 1;
            }
        }
    }
    let mut affected_branch_heads = Vec::new();
    for branch in &graph.branches {
        work.step()?;
        if retired.contains(&branch.head_node_id) {
            affected_branch_heads.push(branch.branch_id.clone());
        }
    }
    let mut last_live_files = selected_files.clone();
    for version in &graph.versions {
        work.step()?;
        if !selected.contains(&version.version_id) {
            last_live_files.remove(&version.file_id);
        }
    }
    let mut archived_branch_heads = BTreeSet::new();
    for file in &last_live_files {
        work.step()?;
        match crate::node_retirement_engine::prospective_archived_branches(
            query.state,
            file.as_str(),
            graph.history_tree_id.as_str(),
        ) {
            Ok(branches) => {
                for branch in branches {
                    work.step()?;
                    let branch = HistoryBranchId::parse(&branch)?;
                    if affected_branch_heads.contains(&branch) {
                        archived_branch_heads.insert(branch);
                    }
                }
            }
            // A known retained namespace/D/path reference leaves its HEAD in
            // the affected set, never in the conditional archive candidates.
            Err(error) if error.code() == ErrorCode::ContentPinned => {}
            Err(error) => return Err(error),
        }
    }
    let frozen_files = selected_files
        .into_iter()
        .filter(|file| query.state.file_deletions.contains_key(file.as_str()))
        .collect();
    work.step()?;
    Ok(PermanentNodeRemovalAnalysis {
        origin_file_id: graph.origin_file_id.clone(),
        history_tree_id: graph.history_tree_id.clone(),
        history_root: graph.history_root,
        retirements,
        file_components,
        parent_projection,
        retained_commit_entries,
        affected_branch_heads,
        archived_branch_heads: archived_branch_heads.into_iter().collect(),
        frozen_files,
        last_live_files: last_live_files.into_iter().collect(),
    })
}

fn components(
    graph: &SharedHistoryGraph,
    selected: &BTreeSet<&HistoryVersionId>,
    work: &mut Work<'_>,
) -> Result<Vec<Vec<HistoryFileId>>> {
    let files = graph
        .files
        .iter()
        .map(|file| file.file_id.as_str())
        .collect();
    let selected = selected.iter().map(|id| id.as_str()).collect();
    let versions = graph
        .versions
        .iter()
        .map(|version| crate::shared_file_topology::Member {
            file: version.file_id.as_str(),
            version: version.version_id.as_str(),
            content: (version.content_hash, version.logical_size),
        });
    crate::shared_file_topology::partition(&files, versions, &selected, || work.step())?
        .into_iter()
        .map(|component| {
            component
                .iter()
                .map(|id| HistoryFileId::parse(id))
                .collect::<storage_contract::Result<_>>()
                .map_err(Into::into)
        })
        .collect()
}

fn parents(
    graph: &SharedHistoryGraph,
    retired: &BTreeSet<SharedHistoryNodeId>,
    work: &mut Work<'_>,
) -> Result<Vec<RemovalParentProjection>> {
    let nodes = graph
        .nodes
        .iter()
        .map(|n| (&n.node_id, n))
        .collect::<BTreeMap<_, _>>();
    let mut remaining = BTreeMap::new();
    let mut children = BTreeMap::<_, Vec<_>>::new();
    for node in &graph.nodes {
        work.step()?;
        remaining.insert(&node.node_id, node.parent_node_ids.len());
        for parent in &node.parent_node_ids {
            work.step()?;
            children.entry(parent).or_default().push(&node.node_id);
        }
    }
    let mut ready = remaining
        .iter()
        .filter_map(|(id, n)| (*n == 0).then_some(*id))
        .collect::<VecDeque<_>>();
    let mut expanded = BTreeMap::<_, BTreeSet<_>>::new();
    let mut expansion_edges = 0;
    let mut output_edges = 0;
    let mut result = BTreeMap::new();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        work.step()?;
        visited += 1;
        let node = nodes[id];
        let mut projected = BTreeSet::new();
        for parent in &node.parent_node_ids {
            work.step()?;
            if retired.contains(parent) {
                for ancestor in &expanded[parent] {
                    work.step()?;
                    projected.insert(*ancestor);
                }
            } else {
                projected.insert(parent);
            }
        }
        if retired.contains(id) {
            if projected.len() > MAX_SHARED_HISTORY_EDGES - expansion_edges {
                return Err(limit());
            }
            expansion_edges += projected.len();
            expanded.insert(id, projected);
        } else {
            if projected.len() > MAX_SHARED_HISTORY_EDGES - output_edges {
                return Err(limit());
            }
            output_edges += projected.len();
            let projected = projected.into_iter().cloned().collect::<Vec<_>>();
            if projected != node.parent_node_ids {
                result.insert(
                    id,
                    RemovalParentProjection {
                        node_id: id.clone(),
                        parent_node_ids: projected,
                    },
                );
            }
        }
        if let Some(children) = children.get(id) {
            for child in children {
                work.step()?;
                let count = remaining
                    .get_mut(child)
                    .expect("invariant: admitted child is indexed");
                *count -= 1;
                if *count == 0 {
                    ready.push_back(child);
                }
            }
        }
    }
    assert_eq!(
        visited,
        nodes.len(),
        "invariant: admitted shared ancestry is acyclic"
    );
    Ok(result.into_values().collect())
}
