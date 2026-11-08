//! Freeze the pre-existing inbound references to retired identities. Cold State
//! admission must not mistake a newly attached old ID for harmless old evidence.
use super::*;

const MAX_REFERENCE_EDGES: usize = 65_536;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct ReferenceClosure {
    digest: String,
    #[serialization(with = "storage_contract::u64_string")]
    edge_count: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serialization(rename_all = "snake_case")]
enum Kind {
    VersionSource,
    CommitEntry,
    CommitPending,
    PathBody,
    PathParent,
    MetadataBody,
    MetadataParent,
    ParentOverride,
    DirectoryEntry,
    DirectoryContext,
}

struct Edges<'a> {
    nodes: BTreeSet<&'a str>,
    values: BTreeSet<(Kind, &'a str, usize, &'a str)>,
}
impl<'a> Edges<'a> {
    fn add(&mut self, kind: Kind, owner: &'a str, position: usize, target: &'a str) -> Result<()> {
        if !self.nodes.contains(target) { return Ok(()); }
        if self.values.len() == MAX_REFERENCE_EDGES {
            return Err(Error::new(ErrorCode::ResourceLimit, "retired-node inbound reference closure is too large"));
        }
        self.values.insert((kind, owner, position, target));
        Ok(())
    }
    fn list(&mut self, kind: Kind, owner: &'a str, targets: &'a [String]) -> Result<()> {
        for (position, target) in targets.iter().enumerate() {
            self.add(kind, owner, position, target)?;
        }
        Ok(())
    }
}

pub(super) fn capture(state: &State, request: &Request) -> Result<ReferenceClosure> {
    let selected = request.version_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut edges = Edges { nodes: BTreeSet::new(), values: BTreeSet::new() };
    for version in state.versions.values() {
        if selected.contains(version.version_id.as_str()) { edges.nodes.insert(&version.version_id); }
    }
    for node in state.file_path_nodes.values() {
        if selected.contains(node.version_id.as_str()) { edges.nodes.insert(&node.node_id); }
    }
    for node in state.file_metadata_nodes.values() {
        if selected.contains(node.version_id.as_str()) { edges.nodes.insert(&node.node_id); }
    }
    if edges.nodes.len() > MAX_SELECTED { return Err(invalid()); }
    for version in state.versions.values() {
        edges.list(Kind::VersionSource, &version.version_id, &version.source_version_ids)?;
    }
    for commit in state.commits.values() {
        for (position, entry) in commit.entries.iter().enumerate() {
            edges.add(Kind::CommitEntry, &commit.commit_id, position, &entry.version_id)?;
        }
        for (position, version) in commit.pending_file_versions.values().enumerate() {
            edges.add(Kind::CommitPending, &commit.commit_id, position, version)?;
        }
    }
    for node in state.file_path_nodes.values() {
        edges.add(Kind::PathBody, &node.node_id, 0, &node.version_id)?;
        edges.list(Kind::PathParent, &node.node_id, &node.parent_node_ids)?;
    }
    for node in state.file_metadata_nodes.values() {
        edges.add(Kind::MetadataBody, &node.node_id, 0, &node.version_id)?;
        edges.list(Kind::MetadataParent, &node.node_id, &node.parent_node_ids)?;
    }
    for (node, parents) in &state.file_node_parents {
        edges.list(Kind::ParentOverride, node, parents)?;
    }
    for tree in state.directory_trees.values() {
        for (position, entry) in tree.entries.iter().enumerate() {
            edges.add(Kind::DirectoryEntry, &tree.tree_id, position, &entry.version_id)?;
        }
        for (position, context) in tree.file_contexts.values().enumerate() {
            if let Some(node) = &context.node_id {
                edges.add(Kind::DirectoryContext, &tree.tree_id, position, node)?;
            }
        }
    }
    Ok(ReferenceClosure {
        digest: crate::object_hash("retired-node-inbound-references", &edges.values)?,
        // The set is capped at 65,536 before insertion.
        edge_count: edges.values.len() as u64,
    })
}
