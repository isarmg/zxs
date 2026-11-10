//! Read-only foreign-disk evidence review. There is no import or prune command.
use crate::storage_directories::{DirectorySource, DiskRoleEvidence};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignDiskPreviewRequest {
    pub source: DirectorySource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignDiskIdentity {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignDiskCheckpoint {
    pub candidate_hash: String,
    pub publication_hash: String,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub authority_hash: Option<String>,
    pub unpublished_tail: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignHistorySummary {
    pub group_id: String,
    pub history_tree_id: String,
    pub repository_name: String,
    pub origin: ForeignDiskIdentity,
    pub manifest_hash: String,
    pub branches: usize,
    pub directory_branches: usize,
    pub directory_trees: usize,
    pub commit_records: usize,
    pub versions: usize,
    pub verified_local_versions: usize,
    /// Missing bodies remain retained history, not invalid branches to prune.
    pub temporarily_unavailable_versions: Vec<String>,
    pub recorded_purged_versions: usize,
    pub recorded_migrated_versions: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ForeignExecutionCapability {
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignDiskPreview {
    pub source: DirectorySource,
    pub identity: ForeignDiskIdentity,
    pub source_level: u32,
    pub role_evidence: DiskRoleEvidence,
    pub seal_digest: String,
    pub review_digest: String,
    pub checkpoint: ForeignDiskCheckpoint,
    pub histories: Vec<ForeignHistorySummary>,
    pub execution: ForeignExecutionCapability,
}
