//! The registered backup forest, admitted once at the disk-list boundary.
//! Each configured source has at most one next-tier successor. Missing sources
//! can remain after deregistration; they are shown without invented edges.
use super::{DecodeError, invalid};
use crate::api;
use data_serialization::Deserialize;
use json_codec::Value;
use std::collections::{BTreeMap, BTreeSet};
use web_interface::i18n::{t, tf};

mod view;
pub(super) use view::view;

#[derive(Debug, error_definition::Error)]
pub enum InvalidTopology {
    #[error("slot {slot} has an invalid tier or upstream role")]
    Role { slot: String },
    #[error("slot {slot} does not immediately follow its source {upstream}")]
    Level { slot: String, upstream: String },
    #[error("source {upstream} has more than one backup successor")]
    Successors { upstream: String },
    #[error("invalid slot identity in backup topology: {0}")]
    Slot(#[source] name_policy::Error),
}

#[derive(Deserialize)]
struct Listing {
    disks: Vec<Disk>,
}

#[derive(Deserialize)]
struct Disk {
    slot_id: String,
    disk_id: String,
    level: u32,
    #[serialization(deserialize_with = "nullable_source")]
    upstream_slot_id: Option<String>,
    online: bool,
}

fn nullable_source<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    Registered(String),
    Missing(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Node {
    pub(super) disk_id: String,
    level: u32,
    online: bool,
    source: Option<Source>,
    row: usize,
    column: usize,
}

impl Node {
    fn tier_label(&self) -> String {
        if self.level == 1 {
            t("主盘 · 第 1 层", "Primary · Tier 1")
        } else {
            tf(
                "备份盘 · 第 {0} 层",
                "Backup disk · Tier {0}",
                &[&self.level.to_string()],
            )
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Edge {
    row: usize,
    column: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Topology {
    pub(super) nodes: BTreeMap<String, Node>,
    edges: BTreeMap<(String, String), Edge>,
    columns: usize,
    levels: usize,
}

impl Topology {
    fn ordered_slots(&self) -> Vec<String> {
        let mut nodes = self.nodes.iter().collect::<Vec<_>>();
        nodes.sort_by_key(|(_, node)| (node.row, node.column));
        nodes.into_iter().map(|(slot, _)| slot.clone()).collect()
    }

    pub(super) fn selection(&self, current: Option<&str>) -> Option<String> {
        current
            .filter(|slot| self.nodes.contains_key(*slot))
            .map(str::to_owned)
            .or_else(|| {
                self.nodes
                    .iter()
                    .min_by_key(|(slot, node)| (node.level, *slot))
                    .map(|(slot, _)| slot.clone())
            })
    }

    fn pending_row(&self) -> usize {
        self.levels * 2 + 1
    }
    fn columns_with_pending(&self, candidates: usize) -> usize {
        self.columns.max(candidates)
    }
    fn rows_with_pending(&self, pending: bool) -> String {
        if pending {
            std::iter::repeat_n("auto", self.levels + 1)
                .collect::<Vec<_>>()
                .join(" var(--zxs-compact-row-height) ")
        } else {
            self.rows_style()
        }
    }
    fn rows_style(&self) -> String {
        std::iter::repeat_n("auto", self.levels)
            .collect::<Vec<_>>()
            .join(" var(--zxs-compact-row-height) ")
    }
}

fn identities(disks: Vec<Disk>) -> Result<BTreeMap<String, Disk>, DecodeError> {
    let mut slots = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut sources = BTreeSet::new();
    for disk in disks {
        name_policy::validate_slot(&disk.slot_id)
            .map_err(|source| DecodeError::Topology(InvalidTopology::Slot(source)))?;
        if disk.disk_id.is_empty() || !ids.insert(disk.disk_id.clone()) {
            return Err(DecodeError::DiskList);
        }
        if disk.level == 0 || (disk.level == 1) != disk.upstream_slot_id.is_none() {
            return Err(DecodeError::Topology(InvalidTopology::Role {
                slot: disk.slot_id,
            }));
        }
        if let Some(upstream) = &disk.upstream_slot_id {
            name_policy::validate_slot(upstream)
                .map_err(|source| DecodeError::Topology(InvalidTopology::Slot(source)))?;
            if !sources.insert(upstream.to_ascii_lowercase()) {
                return Err(DecodeError::Topology(InvalidTopology::Successors {
                    upstream: upstream.clone(),
                }));
            }
        }
        if slots
            .insert(disk.slot_id.to_ascii_lowercase(), disk)
            .is_some()
        {
            return Err(DecodeError::DiskList);
        }
    }
    Ok(slots)
}

fn layout(disks: BTreeMap<String, Disk>) -> Result<Topology, DecodeError> {
    let mut ordered = disks.values().collect::<Vec<_>>();
    ordered.sort_by_key(|disk| (disk.level, &disk.slot_id));
    let rows = ordered
        .iter()
        .map(|disk| disk.level)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .rev()
        .enumerate()
        .map(|(index, level)| (level, index * 2 + 1))
        .collect::<BTreeMap<_, _>>();
    let mut tree = Topology {
        levels: rows.len(),
        ..Default::default()
    };
    for disk in ordered {
        let row = rows[&disk.level];
        let upstream = disk
            .upstream_slot_id
            .as_ref()
            .and_then(|slot| disks.get(&slot.to_ascii_lowercase()));
        let (source, column) = if let Some(upstream) = upstream {
            if upstream.level.checked_add(1) != Some(disk.level) {
                return Err(DecodeError::Topology(InvalidTopology::Level {
                    slot: disk.slot_id.clone(),
                    upstream: upstream.slot_id.clone(),
                }));
            }
            let column = tree.nodes[&upstream.slot_id].column;
            tree.edges.insert(
                (upstream.slot_id.clone(), disk.slot_id.clone()),
                Edge {
                    row: row + 1,
                    column,
                },
            );
            (Some(Source::Registered(upstream.slot_id.clone())), column)
        } else {
            tree.columns += 1;
            (
                disk.upstream_slot_id.clone().map(Source::Missing),
                tree.columns,
            )
        };
        tree.nodes.insert(
            disk.slot_id.clone(),
            Node {
                disk_id: disk.disk_id.clone(),
                level: disk.level,
                online: disk.online,
                source,
                row,
                column,
            },
        );
    }
    Ok(tree)
}

pub(super) fn read(value: Value) -> Result<Topology, api::Error> {
    let listing =
        Listing::deserialize(value).map_err(|source| invalid(DecodeError::Json(source)))?;
    layout(identities(listing.disks).map_err(invalid)?).map_err(invalid)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/disk_tree_tests.rs"]
mod tests;
