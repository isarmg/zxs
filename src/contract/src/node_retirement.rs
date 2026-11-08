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
    pub status: ReplicaState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cleanup {
    Observed { history_root: HistoryRoot, replicas: Vec<Replica> },
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
                        || !seen.insert((&replica.version_id, &replica.slot_id)) { return Err(invalid()); }
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
    pub fn all_current_routes_absent(&self) -> bool {
        matches!(self, Self::Observed { replicas, .. } if !replicas.is_empty() && replicas.iter().all(|r| matches!(r.status, ReplicaState::VerifiedAbsent)))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Outcome {
    pub file_id: HistoryFileId,
    pub operation: json_codec::Value,
    pub cleanup: Cleanup,
}

fn valid_slot(slot: &str) -> bool {
    let bytes = slot.as_bytes();
    bytes.len() <= 64
        && bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
