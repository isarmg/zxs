//! Evidence on one already-authorized FILE timeline. A missing link is not a
//! body identity. No hierarchy is reconstructed from timestamps or names.
use super::{Detail, SourceId};
use crate::{BranchId, FileNodeId, OperationId};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_LINKS: usize = 64;
pub const MAX_INTERVAL_NODES: usize = 128;
pub const MAX_INTERVAL_EDGES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum References { Complete { links: Vec<NodeLink> }, Open {} }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NodeLink {
    pub role: LinkRole,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub node: Option<FileNodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serialization(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum LinkRole {
    Before, After, BranchHead { branch: BranchId }, MergeInput { branch: BranchId }, Chosen, Result,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct EventKey {
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub operation: OperationId,
    pub source: SourceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Interval { pub older: EventKey, pub newer: EventKey, pub body: IntervalBody }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntervalBody {
    Open {},
    Complete { nodes: Vec<IntervalNode>, branches: Vec<BranchTip> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct IntervalNode { pub node: FileNodeId, pub parents: Vec<FileNodeId> }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BranchTip { pub branch: BranchId, pub node: FileNodeId }

fn allowed(node: FileNodeId) -> bool {
    matches!(node, FileNodeId::Version(_) | FileNodeId::Anchor(_))
}

impl References {
    pub(super) fn agrees(&self, detail: &Detail) -> bool {
        let Self::Complete { links } = self else { return true };
        if links.len() > MAX_LINKS || links.iter().any(|r| r.node.is_some_and(|n| !allowed(n))) {
            return false;
        }
        let roles: Vec<_> = links.iter().map(|link| link.role).collect();
        match detail {
            Detail::PathChanged { .. } => roles == [LinkRole::Before, LinkRole::After],
            Detail::Delete { .. } | Detail::Undo { .. } | Detail::Import { .. } =>
                roles.iter().all(|r| matches!(r, LinkRole::BranchHead { .. }))
                    && roles.windows(2).all(|p| p[0] < p[1]),
            Detail::Merged { .. } => {
                let [inputs @ .., chosen, result] = links.as_slice() else { return false };
                inputs.len() >= 2 && inputs.iter().all(|r| matches!(r.role, LinkRole::MergeInput { .. }))
                    && inputs.iter().map(|r| (&r.role, r.node)).collect::<BTreeSet<_>>().len() == inputs.len()
                    && chosen.role == LinkRole::Chosen && result.role == LinkRole::Result && chosen.node == result.node
            }
        }
    }
}

impl Interval {
    pub(super) fn agrees(&self, older: EventKey, newer: EventKey) -> bool {
        if self.older != older || self.newer != newer || older >= newer { return false }
        let IntervalBody::Complete { nodes, branches } = &self.body else { return true };
        if older.sequence >= newer.sequence || nodes.len() > MAX_INTERVAL_NODES || branches.len() > MAX_LINKS
            || nodes.iter().try_fold(0usize, |n, v| n.checked_add(v.parents.len()))
                .is_none_or(|n| n > MAX_INTERVAL_EDGES) { return false }
        let mut index = BTreeMap::new();
        for row in nodes {
            if !allowed(row.node) || index.insert(row.node, &row.parents).is_some()
                || row.parents.iter().any(|p| !allowed(*p) || *p == row.node)
                || row.parents.iter().collect::<BTreeSet<_>>().len() != row.parents.len() { return false }
        }
        if branches.iter().any(|tip| !index.contains_key(&tip.node))
            || branches.iter().map(|b| b.branch).collect::<BTreeSet<_>>().len() != branches.len() { return false }
        // No false complete section on a cycle. Outside parents are real
        // boundary references, not extra in-section nodes to manufacture.
        let mut done = BTreeSet::new();
        loop {
            let before = done.len();
            for (id, parents) in &index {
                if parents.iter().all(|p| !index.contains_key(p) || done.contains(p)) { done.insert(*id); }
            }
            if done.len() == index.len() { return true }
            if done.len() == before { return false }
        }
    }
}
