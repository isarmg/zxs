//! Common identity, ancestry and inherited labels for every public file node.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Explicit nodes from one caller-selected file graph. Visibility and history
/// selection belong to the caller; ancestry retains the single-node semantics.
pub struct FileNodeProjectionRequest<'a> {
    pub file_id: &'a str,
    pub node_ids: &'a [&'a str],
}

/// Derived graph facts only, without exposing the projection's state or cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileNodeSummary {
    pub parent_node_ids: Vec<String>,
    pub tags: BTreeSet<String>,
}

pub(crate) fn node_version(state: &State, file: &str, node: &str) -> Result<String> {
    let version = retained_node_version(state, file, node)?;
    if crate::node_retirement_engine::is_retired(state, version) {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNodeNotFound,
            "node is permanently retired",
        ));
    }
    Ok(version.to_owned())
}

fn retained_node_version<'a>(state: &'a State, file: &str, node: &str) -> Result<&'a str> {
    let version = state
        .versions
        .get(node)
        .filter(|v| v.file_id == file)
        .map(|v| v.version_id.as_str())
        .or_else(|| {
            state
                .file_path_nodes
                .get(node)
                .filter(|n| n.file_id == file)
                .map(|n| n.version_id.as_str())
        })
        .or_else(|| {
            state
                .file_metadata_nodes
                .get(node)
                .filter(|n| n.file_id == file)
                .map(|n| n.version_id.as_str())
        })
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNodeNotFound,
                "node does not belong to this file",
            )
        })?;
    let body = state
        .versions
        .get(version)
        .expect("invariant: admitted file nodes reference an existing body");
    debug_assert_eq!(
        body.file_id, file,
        "invariant: admitted node body belongs to its file"
    );
    Ok(version)
}

pub(crate) fn base_parents(state: &State, node: &str) -> Vec<String> {
    if let Some(n) = state.file_metadata_nodes.get(node) {
        n.parent_node_ids.clone()
    } else if let Some(n) = state.file_path_nodes.get(node) {
        n.parent_node_ids.clone()
    } else if let Some(v) = state.versions.get(node) {
        state
            .file_node_parents
            .get(node)
            .cloned()
            .unwrap_or_else(|| v.source_version_ids.clone())
    } else {
        Vec::new()
    }
}

pub(crate) fn node_parents(state: &State, node: &str) -> Vec<String> {
    let mut parents = base_parents(state, node);
    for event in state
        .file_branch_merges
        .values()
        .filter(|m| m.result_node_id == node)
    {
        for parent in &event.parent_node_ids {
            if parent != node && !parents.contains(parent) {
                parents.push(parent.clone());
            }
        }
    }
    parents
}

fn display_parents(state: &State, parents: &[String]) -> Result<Vec<String>> {
    let mut remaining = 1_048_576usize;
    crate::node_retirement_engine::live_parents(state, parents, || {
        if remaining == 0 {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "retired ancestry traversal exceeds its complete-result budget",
            ));
        }
        remaining -= 1;
        Ok(())
    })
    .map(|parents| parents.into_iter().map(str::to_owned).collect())
}

pub(crate) fn belongs_to_branch(state: &State, branch: &str, head: &str, source: &str) -> bool {
    if state
        .file_metadata_nodes
        .get(source)
        .is_some_and(|n| n.branch_id == branch)
        || state
            .file_path_nodes
            .get(source)
            .is_some_and(|n| n.branch_id == branch)
    {
        return true;
    }
    let mut todo = vec![head.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(node) = todo.pop() {
        if node == source {
            return true;
        }
        if visited.insert(node.clone()) {
            todo.extend(node_parents(state, &node));
        }
    }
    false
}

/// Ordinary child nodes inherit their first parent's immutable tag snapshot.
/// A merge revision stores the explicitly selected result node's tags.
pub(crate) fn node_tags(state: &State, node: &str) -> BTreeSet<String> {
    let mut current = node.to_owned();
    let mut seen = BTreeSet::new();
    while seen.insert(current.clone()) {
        if let Some(n) = state.file_metadata_nodes.get(&current) {
            return n.tags.clone();
        }
        let Some(parent) = base_parents(state, &current).into_iter().next() else {
            break;
        };
        current = parent;
    }
    BTreeSet::new()
}

/// A call-local projection over one immutable generation. Metadata nodes and
/// merge provenance from a later publication must never enter this projection.
pub(crate) struct NodeProjection<'a> {
    state: &'a State,
    tags: std::collections::BTreeMap<String, std::rc::Rc<BTreeSet<String>>>,
    merges: std::collections::BTreeMap<&'a str, Vec<&'a FileBranchMerge>>,
}

impl<'a> NodeProjection<'a> {
    pub(crate) fn new(state: &'a State) -> Self {
        Self::from_merge_events(state, state.file_branch_merges.values())
    }

    pub(crate) fn from_merge_events(
        state: &'a State,
        events: impl IntoIterator<Item = &'a FileBranchMerge>,
    ) -> Self {
        let mut merges = std::collections::BTreeMap::<_, Vec<_>>::new();
        // Preserve the original map iteration order within each result node.
        for event in events {
            merges
                .entry(event.result_node_id.as_str())
                .or_default()
                .push(event);
        }
        Self {
            state,
            tags: Default::default(),
            merges,
        }
    }

    pub(crate) fn parents(&self, node: &str) -> Vec<String> {
        let mut parents = base_parents(self.state, node);
        if let Some(events) = self.merges.get(node) {
            for event in events {
                for parent in &event.parent_node_ids {
                    if parent != node && !parents.contains(parent) {
                        parents.push(parent.clone());
                    }
                }
            }
        }
        parents
    }

    pub(crate) fn tags(&mut self, node: &str) -> &BTreeSet<String> {
        if !self.tags.contains_key(node) {
            let mut current = node;
            let mut path = Vec::new();
            let mut seen = BTreeSet::new();
            let tags = loop {
                if let Some(tags) = self.tags.get(current) {
                    break tags.clone();
                }
                if !seen.insert(current) {
                    break std::rc::Rc::new(BTreeSet::new());
                }
                path.push(current);
                // Metadata wins even for overlapping IDs and cyclic parents.
                if let Some(metadata) = self.state.file_metadata_nodes.get(current) {
                    break std::rc::Rc::new(metadata.tags.clone());
                }
                let parents = if let Some(path) = self.state.file_path_nodes.get(current) {
                    Some(&path.parent_node_ids)
                } else if let Some(version) = self.state.versions.get(current) {
                    Some(
                        self.state
                            .file_node_parents
                            .get(current)
                            .unwrap_or(&version.source_version_ids),
                    )
                } else {
                    None
                };
                let Some(parent) = parents.and_then(|parents| parents.first()) else {
                    break std::rc::Rc::new(BTreeSet::new());
                };
                current = parent;
            };
            // Every visited first-parent prefix has the same snapshot, including
            // dangling tails and cycles without metadata. Share the snapshot.
            for visited in path {
                self.tags.insert(visited.to_owned(), tags.clone());
            }
        }
        self.tags
            .get(node)
            .expect("visited node is memoized")
            .as_ref()
    }

    fn summaries(
        &mut self,
        request: FileNodeProjectionRequest<'_>,
    ) -> BTreeMap<String, Result<FileNodeSummary>> {
        let mut summaries = BTreeMap::new();
        for node in request.node_ids {
            summaries.entry((*node).to_owned()).or_insert_with(|| {
                node_version(self.state, request.file_id, node)?;
                Ok(FileNodeSummary {
                    parent_node_ids: display_parents(self.state, &self.parents(node))?,
                    tags: self.tags(node).clone(),
                })
            });
        }
        summaries
    }
}

pub(crate) fn branch_head(state: &State, branch: &str) -> Result<String> {
    let (file, node) = branch_head_record(state, branch)?;
    node_version(state, file, node)?;
    Ok(node.to_owned())
}

/// A derived audit row may preserve a retired HEAD. This validates its retained
/// ownership, not live-node access or the terminal publication's receipt closure.
pub(crate) fn retained_branch_head(state: &State, branch: &str) -> Result<String> {
    let (file, node) = branch_head_record(state, branch)?;
    retained_node_version(state, file, node)?;
    Ok(node.to_owned())
}

fn branch_head_record<'a>(state: &'a State, branch: &str) -> Result<(&'a str, &'a str)> {
    let branch = state.branches.get(branch).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileBranchNotFound,
            "file branch missing",
        )
    })?;
    let file = branch.file_id.as_deref().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileBranchRequired,
            "an independent file branch is required",
        )
    })?;
    let node = branch
        .file_node_id
        .as_deref()
        .or_else(|| {
            state
                .commits
                .get(&branch.head_commit_id)
                .and_then(|c| {
                    c.entries
                        .iter()
                        .find(|e| e.kind == NamespaceEntryKind::File && e.file_id == file)
                })
                .map(|e| e.version_id.as_str())
        })
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "file branch head is missing",
            )
        })?;
    Ok((file, node))
}

impl MetadataView {
    /// Project against this retained generation, without an Engine borrow.
    pub fn project_file_nodes(
        &self,
        request: FileNodeProjectionRequest<'_>,
    ) -> BTreeMap<String, Result<FileNodeSummary>> {
        NodeProjection::new(self.business_projection()).summaries(request)
    }
}

impl Engine {
    /// Project requested nodes against this immutable state in one batch.
    /// Merge events are scanned once globally, while first-parent tag prefixes
    /// are shared within this call. The result preserves per-node lookup errors;
    /// one invalid node does not suppress other rows. Nothing is retained across
    /// calls or publications, and this does not paginate or validate the graph.
    pub fn project_file_nodes(
        &self,
        request: FileNodeProjectionRequest<'_>,
    ) -> BTreeMap<String, Result<FileNodeSummary>> {
        NodeProjection::new(&self.state).summaries(request)
    }

    pub fn file_node_version(&self, file: &str, node: &str) -> Result<String> {
        node_version(&self.state, file, node)
    }
    pub fn file_node_tags(&self, file: &str, node: &str) -> Result<BTreeSet<String>> {
        node_version(&self.state, file, node)?;
        Ok(node_tags(&self.state, node))
    }
    pub fn file_node_parents(&self, file: &str, node: &str) -> Result<Vec<String>> {
        node_version(&self.state, file, node)?;
        display_parents(&self.state, &node_parents(&self.state, node))
    }
    pub fn file_branch_head(&self, branch: &str) -> Result<String> {
        branch_head(&self.state, branch)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_nodes/projection_tests.rs"]
mod projection_tests;
