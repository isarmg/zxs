//! Common identity and ancestry for every public file node.
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

pub(super) fn retained_node_version<'a>(state: &'a State, file: &str, node: &str) -> Result<&'a str> {
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

/// The SMB name of an explicitly selected immutable FILE node. A reused merge
/// result is the selected node itself, not its independent merge fact. Neither
/// today's container basename nor the physical storage-id leaf is a source.
pub(crate) fn committed_name(
    state: &State, file: storage_contract::FileId, node: storage_contract::FileNodeId,
) -> Result<name_policy::Name> {
    let key = node.to_string();
    let file = file.to_string();
    let version = node_version(state, &file, &key)?;
    let body = &state.versions[&version];
    let missing = || Error::new(storage_contract::ErrorCode::FileNodeNotFound,
        "selected FILE node has no proved committed logical name");
    match node {
        storage_contract::FileNodeId::Version(_) if key == body.version_id => {
            Ok(name_policy::Name::parse(&body.creation_name)?)
        }
        storage_contract::FileNodeId::Anchor(_) => {
            let path = state.file_path_nodes.get(&key).ok_or_else(missing)?;
            if path.node_id != key || path.file_id != file || path.history_tree_id != body.history_tree_id
                || path.version_id != body.version_id
            {
                return Err(missing());
            }
            Ok(name_policy::Name::parse(name_policy::PortablePath::parse(&path.path)?.file_name())?)
        }
        // The native adoption producer does not issue a metadata node. A
        // foreign graph's body or audit label alone cannot attest its name.
        _ => Err(missing()),
    }
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
    base_parents(state, node)
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

/// A call-local projection over one immutable generation. Metadata nodes and
/// merge provenance from a later publication must never enter this projection.
pub(crate) struct NodeProjection<'a> {
    state: &'a State,
}

impl<'a> NodeProjection<'a> {
    pub(crate) fn new(state: &'a State) -> Self {
        Self { state }
    }

    pub(crate) fn parents(&self, node: &str) -> Vec<String> {
        base_parents(self.state, node)
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
    /// An independent merge decision never contributes immutable content edges.
    /// The result preserves per-node lookup errors;
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
