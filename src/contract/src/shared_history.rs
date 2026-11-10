//! A complete, bounded same-history component. Content relationships do not
//! replace file/event identities or confer write authority or body availability.
use crate::history_window::{
    HistoryBranchId, HistoryCollection, HistoryFileId, HistoryNodeId, HistoryPageLimit,
    HistoryRoot, HistoryTreeId, HistoryVersionId, HistoryWindowRequest,
};
use crate::{BranchState, ContentHash, ContentState, Error, ErrorCode, Result};
use data_serialization::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_SHARED_HISTORY_FILES: usize = 64;
pub const MAX_SHARED_HISTORY_NODES: usize = 512;
pub const MAX_SHARED_HISTORY_BRANCHES: usize = 256;
pub const MAX_SHARED_HISTORY_EDGES: usize = 2048;
pub const MAX_SHARED_HISTORY_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedHistoryRequest {
    pub file_id: HistoryFileId,
    pub history_root: HistoryRoot,
}
impl SharedHistoryRequest {
    pub fn parse(file: &str, root: &str) -> Result<Self> {
        Ok(Self {
            file_id: HistoryFileId::parse(file)?,
            history_root: HistoryRoot::parse(root)?,
        })
    }
    pub fn route(&self) -> String {
        format!("/api/file-manager/files/{}/shared-history", self.file_id)
    }
    pub fn query_fields(&self) -> BTreeMap<String, String> {
        BTreeMap::from([("history_root".into(), self.history_root.to_string())])
    }
    pub fn history_request(&self) -> HistoryWindowRequest {
        HistoryWindowRequest {
            file_id: self.file_id.clone(),
            collection: HistoryCollection::Nodes,
            location: None,
            history_root: Some(self.history_root),
            cursor: None,
            anchor_node_id: None,
            limit: HistoryPageLimit::for_collection(1, &HistoryCollection::Nodes)
                .expect("invariant: one node is within history policy"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serialization(transparent)]
pub struct SharedHistoryNodeId(String);
impl SharedHistoryNodeId {
    pub fn parse(value: &str) -> Result<Self> {
        if !value.starts_with("fdel_") {
            HistoryNodeId::parse(value)?;
        } else {
            crate::validate_id(value, "fdel")?;
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for SharedHistoryNodeId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::parse(&String::deserialize(d)?).map_err(data_serialization::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum SharedHistoryNodeKind {
    Content,
    Rename,
    Tags,
    Merge,
    Deletion,
}
impl<'de> Deserialize<'de> for SharedHistoryNodeKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "content" => Ok(Self::Content),
            "rename" => Ok(Self::Rename),
            "tags" => Ok(Self::Tags),
            "merge" => Ok(Self::Merge),
            "deletion" => Ok(Self::Deletion),
            value => Err(data_serialization::de::Error::unknown_variant(
                value,
                &["content", "rename", "tags", "merge", "deletion"],
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SharedHistoryFile {
    pub file_id: HistoryFileId,
    /// Name on this file's first version in stable version-ID order, not a path.
    pub creation_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SharedHistoryVersion {
    pub version_id: HistoryVersionId,
    pub file_id: HistoryFileId,
    pub creation_name: String,
    #[serialization(with = "crate::u64_string")]
    pub logical_size: u64,
    pub content_hash: ContentHash,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub content_state: Option<ContentState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SharedHistoryNode {
    pub node_id: SharedHistoryNodeId,
    pub file_id: HistoryFileId,
    pub kind: SharedHistoryNodeKind,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub version_id: Option<HistoryVersionId>,
    /// Retained record sequence, not an ordering proof between different files.
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub parent_node_ids: Vec<SharedHistoryNodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SharedHistoryBranch {
    pub branch_id: HistoryBranchId,
    pub file_id: HistoryFileId,
    pub name: String,
    pub head_node_id: SharedHistoryNodeId,
    pub state: BranchState,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub merged_into: Option<HistoryBranchId>,
}

/// A non-temporal equality hub. All versions keep their original event IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SharedContent {
    #[serialization(with = "crate::u64_string")]
    pub logical_size: u64,
    pub content_hash: ContentHash,
    pub version_ids: Vec<HistoryVersionId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SharedHistoryGraph {
    pub origin_file_id: HistoryFileId,
    pub history_tree_id: HistoryTreeId,
    pub history_root: HistoryRoot,
    pub files: Vec<SharedHistoryFile>,
    pub versions: Vec<SharedHistoryVersion>,
    pub nodes: Vec<SharedHistoryNode>,
    pub branches: Vec<SharedHistoryBranch>,
    pub shared_contents: Vec<SharedContent>,
}

impl SharedHistoryGraph {
    /// Validate an untrusted complete component, its ancestry and equality
    /// relations. No truncation flag or missing edge can masquerade as complete.
    pub fn validate(&self, request: &SharedHistoryRequest) -> Result<()> {
        if self.origin_file_id != request.file_id
            || self.history_root != request.history_root
            || self.files.is_empty()
            || self.files.len() > MAX_SHARED_HISTORY_FILES
            || self.versions.len() > MAX_SHARED_HISTORY_NODES
            || self.nodes.len() > MAX_SHARED_HISTORY_NODES
            || self.branches.len() > MAX_SHARED_HISTORY_BRANCHES
            || self.shared_contents.len() > MAX_SHARED_HISTORY_NODES
        {
            return Err(invalid(
                "shared history identity or collection limit differs",
            ));
        }
        let files = ordered(&self.files, |file| &file.file_id)?;
        let versions = ordered(&self.versions, |version| &version.version_id)?;
        let nodes = ordered(&self.nodes, |node| &node.node_id)?;
        let branches = ordered(&self.branches, |branch| &branch.branch_id)?;
        if !files.contains_key(&self.origin_file_id) {
            return Err(invalid("shared history omits its origin file"));
        }
        let mut representatives = BTreeMap::new();
        for version in &self.versions {
            name_policy::validate_name(&version.creation_name)?;
            representatives.entry(&version.file_id).or_insert(version);
            // HistoryVersionId already proves the stricter content-event syntax.
            let id = SharedHistoryNodeId(version.version_id.to_string());
            if !files.contains_key(&version.file_id)
                || nodes
                    .get(&id)
                    .is_none_or(|node| node.kind != SharedHistoryNodeKind::Content)
            {
                return Err(invalid("shared version has no file or content event"));
            }
        }
        for file in &self.files {
            name_policy::validate_name(&file.creation_name)?;
            if representatives
                .get(&file.file_id)
                .is_none_or(|version| version.creation_name != file.creation_name)
            {
                return Err(invalid("shared history file has no representative version"));
            }
        }
        self.validate_nodes(&files, &versions, &nodes)?;
        for branch in &self.branches {
            crate::BranchName::parse(&branch.name)?;
            if nodes.get(&branch.head_node_id).is_none_or(|node| {
                node.file_id != branch.file_id || node.kind == SharedHistoryNodeKind::Deletion
            }) {
                return Err(invalid("shared branch has an invalid head"));
            }
        }
        validate_aliases(&branches)?;
        self.validate_content_relations(&versions)?;
        Ok(())
    }

    fn validate_nodes(
        &self,
        files: &BTreeMap<&HistoryFileId, &SharedHistoryFile>,
        versions: &BTreeMap<&HistoryVersionId, &SharedHistoryVersion>,
        nodes: &BTreeMap<&SharedHistoryNodeId, &SharedHistoryNode>,
    ) -> Result<()> {
        let mut edge_count = 0;
        let mut remaining = BTreeMap::new();
        let mut children = BTreeMap::<_, Vec<_>>::new();
        for node in &self.nodes {
            let expected_prefix = match node.kind {
                SharedHistoryNodeKind::Content => "ver_",
                SharedHistoryNodeKind::Rename => "fnode_",
                SharedHistoryNodeKind::Tags | SharedHistoryNodeKind::Merge => "fmeta_",
                SharedHistoryNodeKind::Deletion => "fdel_",
            };
            if !files.contains_key(&node.file_id)
                || !node.node_id.as_str().starts_with(expected_prefix)
                || (node.kind == SharedHistoryNodeKind::Deletion) != node.version_id.is_none()
                || node.version_id.as_ref().is_some_and(|id| {
                    versions
                        .get(id)
                        .is_none_or(|body| body.file_id != node.file_id)
                        || (node.kind == SharedHistoryNodeKind::Content
                            && node.node_id.as_str() != id.as_str())
                })
            {
                return Err(invalid("shared event has an invalid identity or body"));
            }
            if node.parent_node_ids.len() > MAX_SHARED_HISTORY_EDGES - edge_count {
                return Err(invalid("shared history exceeds its parent-edge limit"));
            }
            edge_count += node.parent_node_ids.len();
            let mut previous = None;
            for parent in &node.parent_node_ids {
                if previous.is_some_and(|previous| previous >= parent)
                    || parent == &node.node_id
                    || nodes.get(parent).is_none_or(|parent| {
                        parent.file_id != node.file_id
                            || parent.kind == SharedHistoryNodeKind::Deletion
                    })
                {
                    return Err(invalid(
                        "shared ancestry is unordered, missing or crosses files",
                    ));
                }
                previous = Some(parent);
                children.entry(parent).or_default().push(&node.node_id);
            }
            remaining.insert(&node.node_id, node.parent_node_ids.len());
        }
        let mut ready = remaining
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(*id))
            .collect::<VecDeque<_>>();
        let mut visited = 0;
        while let Some(id) = ready.pop_front() {
            visited += 1;
            if let Some(children) = children.get(id) {
                for child in children {
                    let count = remaining
                        .get_mut(child)
                        .expect("invariant: child was indexed");
                    *count -= 1;
                    if *count == 0 {
                        ready.push_back(child);
                    }
                }
            }
        }
        if visited != nodes.len() {
            return Err(invalid("shared ancestry contains a cycle"));
        }
        Ok(())
    }

    fn validate_content_relations(
        &self,
        versions: &BTreeMap<&HistoryVersionId, &SharedHistoryVersion>,
    ) -> Result<()> {
        let mut expected = BTreeMap::<_, Vec<_>>::new();
        for version in &self.versions {
            if version.logical_size != 0 {
                expected
                    .entry((version.content_hash, version.logical_size))
                    .or_default()
                    .push(version.version_id.clone());
            }
        }
        expected.retain(|_, ids| {
            ids.iter()
                .map(|id| &versions[id].file_id)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        });
        let expected = expected
            .into_iter()
            .map(
                |((content_hash, logical_size), version_ids)| SharedContent {
                    logical_size,
                    content_hash,
                    version_ids,
                },
            )
            .collect::<Vec<_>>();
        if self.shared_contents != expected {
            return Err(invalid(
                "shared content relations are missing, unequal or duplicated",
            ));
        }
        let mut memberships = BTreeMap::<_, BTreeSet<_>>::new();
        for (index, shared) in self.shared_contents.iter().enumerate() {
            for id in &shared.version_ids {
                memberships
                    .entry(&versions[id].file_id)
                    .or_default()
                    .insert(index);
            }
        }
        let mut reached = BTreeSet::from([&self.origin_file_id]);
        let mut pending = VecDeque::from([&self.origin_file_id]);
        let mut expanded = BTreeSet::new();
        while let Some(file) = pending.pop_front() {
            if let Some(groups) = memberships.get(file) {
                for &group in groups {
                    if expanded.insert(group) {
                        for id in &self.shared_contents[group].version_ids {
                            let file = &versions[id].file_id;
                            if reached.insert(file) {
                                pending.push_back(file);
                            }
                        }
                    }
                }
            }
        }
        if reached.len() != self.files.len() {
            return Err(invalid("shared history contains a disconnected file"));
        }
        Ok(())
    }
}

fn validate_aliases(branches: &BTreeMap<&HistoryBranchId, &SharedHistoryBranch>) -> Result<()> {
    for branch in branches.values() {
        let Some(mut cursor) = branch.merged_into.as_ref() else {
            continue;
        };
        if branch.state != BranchState::Merged {
            return Err(invalid("shared alias source is not merged"));
        }
        let mut seen = BTreeSet::from([&branch.branch_id]);
        loop {
            if !seen.insert(cursor) {
                return Err(invalid("shared branch aliases contain a cycle"));
            }
            let target = branches
                .get(cursor)
                .ok_or_else(|| invalid("shared alias target is absent"))?;
            if target.file_id != branch.file_id {
                return Err(invalid("shared alias crosses files"));
            }
            match &target.merged_into {
                Some(next) => cursor = next,
                None => {
                    if target.state != BranchState::Active {
                        return Err(invalid("shared alias does not end at an active branch"));
                    }
                    break;
                }
            }
        }
    }
    Ok(())
}

fn ordered<T, K: Ord>(items: &[T], key: impl Fn(&T) -> &K) -> Result<BTreeMap<&K, &T>> {
    let mut result = BTreeMap::new();
    let mut previous = None;
    for item in items {
        let id = key(item);
        if previous.is_some_and(|previous| previous >= id) {
            return Err(invalid(
                "shared history identities must be distinct and ordered",
            ));
        }
        previous = Some(id);
        result.insert(id, item);
    }
    Ok(result)
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/shared_history.rs"]
mod tests;
