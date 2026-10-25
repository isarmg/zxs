//! Single-writer full-file storage, durable publication, and offline recovery.
//!
//! This is a trusted local domain API, not an authentication boundary. Servers
//! authenticate the sole administrator before calling it. Operation principals
//! bind audit authorship, workspace ownership and idempotency, never repository ACLs.
#![forbid(unsafe_code)]
mod business;
mod container_storage;
mod degraded_recovery;
mod disk;
mod engine;
mod historical_save;
pub use engine::{PreparedWorkspaceCommit, WorkspaceCommitPreparation};
mod history_package;
mod journal;
mod maintenance;
mod metadata_budget;
mod protection;
mod publication_evidence;
mod repair;
mod wire;
pub use business::{
    FileBranchMergeRequest, FileLocation, FileNodeProjectionRequest, FileNodeSummary,
    FileNodeTagsUpdateRequest, PreparedUpload, UploadPreparation, validate_branch_name,
};
pub use degraded_recovery::RecoveryPlan;
pub use disk::{offline_check, offline_export};
pub use engine::{
    ContentReadPlan, DiskObservation, DiskObservationPlan, Engine, OpenedContent,
    OperationReadPlan, OperationRecord, RangeContent, ReadLease, RetainedWorkspaceLimits,
    WorkspaceReadPlan,
};
pub use historical_save::HistoricalSaveOptions;
pub use history_package::{HistoryManifest, HistoryPackageReport, offline_check_history_package};
pub use protection::{
    BackupConfigurationRequest, PlacementRuleUpdate, ProtectionPolicyUpdate,
    ScopedBackupConfigurationRequest,
};
pub use storage_contract::*;
pub use wire::{commit_request_digest, manifest_hash as workspace_manifest_hash};
#[cfg(test)]
#[path = "../../../test/engine/unit/tests.rs"]
mod tests;
