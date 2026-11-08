//! Shared structural route closure. This type proves ancestry only, never
//! membership, disk availability, payload completeness or writer authority.
use crate::{Error, ErrorCode, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(crate) struct Node<'a> {
    pub(crate) slot: &'a str,
    pub(crate) upstream: Option<&'a str>,
    pub(crate) level: u32,
}

pub(crate) struct Topology<'a> {
    nodes: BTreeMap<String, Node<'a>>,
    children: BTreeMap<String, Vec<Node<'a>>>,
}

impl<'a> Topology<'a> {
    pub(crate) fn new(nodes: impl IntoIterator<Item = Node<'a>>) -> Result<Self> {
        let mut exact = BTreeMap::new();
        let mut children = BTreeMap::<String, Vec<Node<'a>>>::new();
        for node in nodes {
            if exact.insert(node.slot.to_ascii_lowercase(), node).is_some() || node.level == 0 {
                return Err(invalid("route repeats a slot identity or has level zero"));
            }
            if let Some(upstream) = node.upstream {
                children
                    .entry(upstream.to_ascii_lowercase())
                    .or_default()
                    .push(node);
            }
        }
        Ok(Self {
            nodes: exact,
            children,
        })
    }

    fn node(&self, slot: &str) -> Result<Node<'a>> {
        self.nodes
            .get(&slot.to_ascii_lowercase())
            .copied()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::SlotNotFound,
                    "route slot is absent from its exact topology",
                )
            })
    }

    /// Legacy copy-count requests can choose only a unique linear prefix.
    /// Encountering a fork is not permission to take the first configured disk,
    /// select all peers, or infer a new replica count.
    pub(crate) fn unique_prefix(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        let mut disk = self.node(primary)?;
        if disk.level != 1 || disk.upstream.is_some() || copies == 0 {
            return Err(Error::new(
                ErrorCode::PlacementInvalid,
                "placement requires a primary and a nonzero explicit copy count",
            ));
        }
        let mut result = vec![disk.slot.to_owned()];
        while result.len() < copies as usize {
            let next = match self
                .children
                .get(&disk.slot.to_ascii_lowercase())
                .map(Vec::as_slice)
            {
                Some([one]) => *one,
                Some(_) => {
                    return Err(Error::new(
                        ErrorCode::RouteIncomplete,
                        "copy count cannot choose between downstream disks; a complete explicit route decision is required",
                    ));
                }
                None => {
                    return Err(Error::new(
                        ErrorCode::RouteIncomplete,
                        "requested copies exceed the configured unique route",
                    ));
                }
            };
            if disk.level.checked_add(1) != Some(next.level) {
                return Err(invalid("route skips a direct upstream level"));
            }
            result.push(next.slot.to_owned());
            disk = next;
        }
        Ok(result)
    }

    /// The caller already chose this exact set. Require every lower-level
    /// ancestor in the same set, then return stable parent-before-child order.
    /// Other topology branches are not probed or made new I/O obligations.
    pub(crate) fn explicit(&self, primary: &str, selected: &[String]) -> Result<Vec<String>> {
        let root = self.node(primary)?;
        if root.level != 1 || root.upstream.is_some() || selected.is_empty() {
            return Err(invalid("explicit route has no complete primary root"));
        }
        let mut exact = BTreeSet::new();
        let mut result = Vec::with_capacity(selected.len());
        for slot in selected {
            let node = self.node(slot)?;
            if !exact.insert(node.slot.to_ascii_lowercase()) {
                return Err(invalid("explicit route repeats a selected disk"));
            }
            result.push(node);
        }
        if !exact.contains(&root.slot.to_ascii_lowercase()) {
            return Err(invalid("explicit route omits its primary"));
        }
        for node in &result {
            if node.slot == root.slot {
                continue;
            }
            let ancestors = self.upstream_chain(node.slot)?;
            if ancestors.last().is_none_or(|last| last.slot != root.slot)
                || ancestors
                    .iter()
                    .any(|parent| !exact.contains(&parent.slot.to_ascii_lowercase()))
            {
                return Err(invalid(
                    "explicit route omits a lower-level ancestor or mixes primaries",
                ));
            }
        }
        result.sort_by(|left, right| {
            left.level.cmp(&right.level).then_with(|| {
                left.slot
                    .to_ascii_lowercase()
                    .cmp(&right.slot.to_ascii_lowercase())
            })
        });
        Ok(result
            .into_iter()
            .map(|node| node.slot.to_owned())
            .collect())
    }

    /// Immediate upstream first, primary last. Only this selected chain is
    /// traversed; an unrelated disabled/missing peer does not become required.
    pub(crate) fn upstream_chain(&self, slot: &str) -> Result<Vec<Node<'a>>> {
        let mut disk = self.node(slot)?;
        let mut result = Vec::new();
        while disk.level > 1 {
            let parent = self.node(
                disk.upstream
                    .ok_or_else(|| invalid("backup lacks upstream"))?,
            )?;
            if parent.level.checked_add(1) != Some(disk.level) {
                return Err(invalid(
                    "route has a cycle or skips a direct upstream level",
                ));
            }
            result.push(parent);
            disk = parent;
        }
        if disk.upstream.is_some() {
            return Err(invalid("primary route has an upstream"));
        }
        Ok(result)
    }
}

fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::TopologyInvalid, detail)
}
