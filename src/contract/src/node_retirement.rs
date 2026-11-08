//! Bounded detail-action transport. Preview is not execution admission or erase proof.
use crate::{Error, ErrorCode, Result};
use crate::history_window::{HistoryBranchId, HistoryFileId, HistoryRoot, HistoryTreeId, HistoryVersionId};
use crate::shared_history::{MAX_SHARED_HISTORY_FILES, MAX_SHARED_HISTORY_NODES, SharedHistoryNodeId};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const CONFIRMATION: &str = "PERMANENTLY_RETIRE_NODES";
pub const MAX_REPLICAS: usize = 4096;

/// Aggregate selection validation happens at ingress, before backend dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Selection {
    file_id: HistoryFileId,
    history_root: HistoryRoot,
    version_ids: Vec<HistoryVersionId>,
}
impl Selection {
    pub fn new(file_id: HistoryFileId, history_root: HistoryRoot, mut version_ids: Vec<HistoryVersionId>) -> Result<Self> {
        if version_ids.is_empty() || version_ids.len() > MAX_SHARED_HISTORY_NODES { return Err(invalid()); }
        version_ids.sort();
        if version_ids.windows(2).any(|pair| pair[0] == pair[1]) { return Err(invalid()); }
        Ok(Self { file_id, history_root, version_ids })
    }
    pub fn file_id(&self) -> &HistoryFileId { &self.file_id }
    pub fn history_root(&self) -> HistoryRoot { self.history_root }
    pub fn version_ids(&self) -> &[HistoryVersionId] { &self.version_ids }
    pub fn route(&self) -> String { format!("/api/file-manager/files/{}/node-retirement", self.file_id) }
}
fn invalid() -> Error { Error::new(ErrorCode::InvalidRequest, "invalid or mismatched node retirement selection/observation") }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Preview {
    pub file_id: HistoryFileId,
    pub history_tree_id: HistoryTreeId,
    pub history_root: HistoryRoot,
    pub version_ids: Vec<HistoryVersionId>,
    pub coupled_node_ids: Vec<SharedHistoryNodeId>,
    pub file_components: Vec<Vec<HistoryFileId>>,
    #[serialization(with = "crate::u64_string")]
    pub retained_commit_entries: u64,
    pub affected_branch_heads: Vec<HistoryBranchId>,
    pub frozen_files: Vec<HistoryFileId>,
}
impl Preview {
    pub fn validate(&self, selection: &Selection) -> Result<()> {
        let files = self.file_components.iter().flatten().collect::<BTreeSet<_>>();
        let count = self.file_components.iter().map(Vec::len).sum::<usize>();
        let nodes = self.coupled_node_ids.iter().collect::<BTreeSet<_>>();
        let heads = self.affected_branch_heads.iter().collect::<BTreeSet<_>>();
        let frozen = self.frozen_files.iter().collect::<BTreeSet<_>>();
        if self.file_id != *selection.file_id() || self.history_root != selection.history_root()
            || self.version_ids != selection.version_ids() || self.coupled_node_ids.len() > MAX_SHARED_HISTORY_NODES
            || self.coupled_node_ids.is_empty() || nodes.len() != self.coupled_node_ids.len()
            || heads.len() != self.affected_branch_heads.len() || frozen.len() != self.frozen_files.len()
            || !frozen.is_subset(&files) || self.file_components.is_empty()
            || self.file_components.iter().any(Vec::is_empty) || files.len() != count || count > MAX_SHARED_HISTORY_FILES
            || !files.contains(selection.file_id()) || self.frozen_files.len() > MAX_SHARED_HISTORY_FILES
            || self.affected_branch_heads.len() > crate::shared_history::MAX_SHARED_HISTORY_BRANCHES {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplicaState {
    NotRequested,
    MissingTombstone,
    PayloadPresent,
    VerifiedAbsent,
    Unverified { error_code: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Replica {
    pub version_id: HistoryVersionId,
    pub slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
    pub original_requirement: bool,
    pub status: ReplicaState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cleanup {
    Observed { history_root: HistoryRoot, scope: CleanupScope, replicas: Vec<Replica> },
    Unverified { error_code: String },
}
impl Cleanup {
    pub fn validate(&self, versions: &[HistoryVersionId]) -> Result<()> {
        match self {
            Self::Unverified { error_code } if !error_code.is_empty() && error_code.len() <= 128 => Ok(()),
            Self::Observed { replicas, .. } if !replicas.is_empty() && replicas.len() <= MAX_REPLICAS => {
                let mut seen = BTreeSet::new();
                let mut covered = BTreeSet::new();
                for replica in replicas {
                    if !versions.contains(&replica.version_id) || !valid_slot(&replica.slot_id)
                        || !seen.insert((&replica.version_id, &replica.slot_id, replica.binding_generation)) { return Err(invalid()); }
                    if let ReplicaState::Unverified { error_code } = &replica.status
                        && (error_code.is_empty() || error_code.len() > 128) { return Err(invalid()); }
                    covered.insert(&replica.version_id);
                }
                if covered.len() != versions.len() { return Err(invalid()); }
                Ok(())
            }
            _ => Err(invalid()),
        }
    }
    pub fn all_required_bindings_absent(&self) -> bool {
        matches!(self, Self::Observed { scope: CleanupScope::OriginalAndCurrentRoutes, replicas, .. }
            if !replicas.is_empty() && replicas.iter().any(|r| r.original_requirement)
            && replicas.iter().all(|r| matches!(r.status, ReplicaState::VerifiedAbsent)))
    }

    /// A reply may not omit an offline original copy and certify only today's
    /// replacement route. The exact original binding set comes from the receipt.
    pub fn validate_for_receipt(&self, versions: &[HistoryVersionId], receipt: &json_codec::Value) -> Result<()> {
        self.validate(versions)?;
        let Self::Observed { scope, replicas, .. } = self else { return Ok(()); };
        if *scope != CleanupScope::OriginalAndCurrentRoutes { return Err(invalid()); }
        let required: RequiredBindings = json_codec::from_value(
            receipt.get("result").and_then(|v| v.get("retirement"))
                .and_then(|v| v.get("cleanup_routes")).ok_or_else(invalid)?.clone(),
        )?;
        if required.bindings.is_empty() || required.bindings.len() > MAX_REPLICAS
            || required.bindings.windows(2).any(|pair| pair[0] >= pair[1]
                || (pair[0].version_id == pair[1].version_id && pair[0].slot_id == pair[1].slot_id)) {
            return Err(invalid());
        }
        let mut covered = BTreeSet::new();
        for binding in &required.bindings {
            if !versions.contains(&binding.version_id) || !valid_slot(&binding.slot_id) { return Err(invalid()); }
            covered.insert(&binding.version_id);
        }
        if covered.len() != versions.len() { return Err(invalid()); }
        let required = required.bindings.iter().map(|b| (&b.version_id, b.slot_id.as_str(), b.binding_generation)).collect::<BTreeSet<_>>();
        let reported = replicas.iter().filter(|r| r.original_requirement)
            .map(|r| (&r.version_id, r.slot_id.as_str(), r.binding_generation)).collect::<BTreeSet<_>>();
        if required != reported { return Err(invalid()); }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Outcome {
    pub file_id: HistoryFileId,
    pub operation: json_codec::Value,
    pub separation: Separation,
    pub cleanup: Cleanup,
}

fn valid_slot(slot: &str) -> bool {
    let bytes = slot.as_bytes();
    bytes.len() <= 64
        && bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Permanent publication-time partition, not a fresh current-topology result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Separation {
    Published {
        history_root: HistoryRoot,
        original_files: Vec<HistoryFileId>,
        file_components: Vec<Vec<HistoryFileId>>,
    },
    Unverified { error_code: String },
}
impl Separation {
    pub fn validate(&self, selection: &Selection) -> Result<()> {
        match self {
            Self::Unverified { error_code } if !error_code.is_empty() && error_code.len() <= 128 => Ok(()),
            Self::Published { history_root, original_files, file_components } => {
                if *history_root != selection.history_root() || original_files.is_empty()
                    || original_files.len() > MAX_SHARED_HISTORY_FILES
                    || original_files.windows(2).any(|pair| pair[0] >= pair[1])
                    || original_files.binary_search(selection.file_id()).is_err()
                    || file_components.is_empty() || file_components.len() > MAX_SHARED_HISTORY_FILES {
                    return Err(invalid());
                }
                let mut files = BTreeSet::new();
                for component in file_components {
                    if component.is_empty() || component.len() > MAX_SHARED_HISTORY_FILES
                        || component.windows(2).any(|pair| pair[0] >= pair[1]) {
                        return Err(invalid());
                    }
                    for file in component {
                        if !files.insert(file) { return Err(invalid()); }
                    }
                }
                if files.into_iter().ne(original_files.iter()) { return Err(invalid()); }
                Ok(())
            }
            _ => Err(invalid()),
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum CleanupScope { CurrentRoutes, OriginalAndCurrentRoutes }
impl<'de> Deserialize<'de> for CleanupScope {
    fn deserialize<D: data_serialization::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "current_routes" => Ok(Self::CurrentRoutes),
            "original_and_current_routes" => Ok(Self::OriginalAndCurrentRoutes),
            _ => Err(data_serialization::de::Error::custom("invalid cleanup observation scope")),
        }
    }
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct RequiredBindings { bindings: Vec<RequiredBinding> }
#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serialization(deny_unknown_fields)]
struct RequiredBinding {
    version_id: HistoryVersionId,
    slot_id: String,
    #[serialization(with = "crate::u64_string")]
    binding_generation: u64,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/node_retirement_tests.rs"]
mod tests;
