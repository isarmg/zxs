//! Single-writer full-file storage, durable publication, and offline recovery.
//!
//! This is a trusted local domain API, not an authentication boundary. Servers
//! authenticate the sole administrator before calling it. Operation principals
//! bind audit authorship, workspace ownership and idempotency, never repository ACLs.
#![forbid(unsafe_code)]
mod error;
pub use error::{
    CheckpointError, Error, PublishedRecoveryReconciliation, PublishedWorkspaceReconciliation,
    RecoveryReconciliationStage,
};
pub type Result<T> = std::result::Result<T, Error>;
mod business;
mod container_storage;
mod degraded_recovery;
mod disk;
mod engine;
mod file_stamp;
mod historical_save;
pub use engine::{PreparedWorkspaceCommit, WorkspaceCommitPreparation};
mod history_package;
mod journal;
#[cfg(feature = "fault-injection")]
pub use journal::JournalFault;
mod maintenance;
mod metadata_budget;
mod metadata_map;
mod namespace;
mod protection;
mod publication_evidence;
mod repair;
mod segment_store;
mod state_projection;
mod wire;
pub use business::{
    FileBranchMergeRequest, FileNodeProjectionRequest, FileNodeSummary, FileNodeTagsUpdateRequest,
    NamespaceTree, PreparedUpload, UploadPreparation, file_history_pages, history_graph_pages,
    validate_branch_name,
};
pub use degraded_recovery::RecoveryPlan;
pub use disk::{offline_check, offline_export};
#[cfg(feature = "fault-injection")]
pub use engine::MemberFault;
pub use engine::{
    ContentReadPlan, DiskObservation, DiskObservationPlan, Engine, EventPageReadPlan, HistoryRoot,
    IndexFailure, IndexFailureStage, IndexHealth, IntegrityScrubArea, IntegrityScrubBatch,
    IntegrityScrubBatchAdmission, IntegrityScrubCancellation, IntegrityScrubCompletion,
    IntegrityScrubIssue, IntegrityScrubOutcome, IntegrityScrubPlan, IntegrityScrubReport,
    IntegrityScrubSchedule, IntegrityScrubTarget, MetadataMemberProbe, OpenedContent,
    OperationLookup, OperationReadObservation, OperationReadPlan, OperationRecord,
    PublicationStageReport, PublishedReadView, RangeContent, ReadLease, RecoverySealContext,
    RecoverySealFailure, RecoveryWarnings, RetainedWorkspaceLimits, StageIo, StageKind,
    StageTiming, ValidatedConfig, WorkspaceInventory, WorkspaceInventoryRecord, WorkspaceReadPlan,
};
pub use history_package::{HistoryManifest, HistoryPackageReport, offline_check_history_package};
pub use protection::{
    BackupConfigurationRequest, PlacementRuleUpdate, ProtectionPolicyUpdate,
    ScopedBackupConfigurationRequest,
};
pub use storage_contract::*;
pub use wire::{commit_request_digest, manifest_hash as workspace_manifest_hash};

/// Test-only fixture support for dependent crates' unit tests. Gated behind
/// the `test-support` feature, which production builds never enable. Nothing
/// here relaxes admission: planted objects are canonical encodings, and a
/// replaced state is the caller's responsibility to keep admitted-shaped.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use crate::engine::Engine;

    /// Build the canonical paged namespace of `entries` (absent parent
    /// directories are synthesized deterministically) and store its objects
    /// onto this engine's member disks, so hand-built commit fixtures resolve
    /// through the same bounded reads as published commits.
    pub fn plant_namespace(
        engine: &Engine,
        entries: &[storage_contract::Entry],
    ) -> crate::Result<storage_contract::CommitNamespace> {
        let (namespace, mut objects) = crate::namespace::fixture_namespace(entries);
        let members = engine.members();
        for object in objects.take() {
            object.store(&members)?;
        }
        Ok(namespace)
    }

    /// Replace the engine's in-memory state with a hand-mutated fixture copy.
    /// The caller owns consistency with planted namespace objects and derived
    /// indexes; reads served from the replaced state are fixture reads.
    pub fn replace_state(engine: &mut Engine, state: storage_contract::State) {
        engine.state = std::sync::Arc::new(state);
    }
}
#[cfg(test)]
#[path = "../../../test/engine/unit/journal/r29_local_publication.rs"]
mod r29_local_publication;
#[cfg(test)]
#[path = "../../../test/engine/unit/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/canonical_publication_fixture.rs"]
mod canonical_publication_fixture;

#[cfg(test)]
#[path = "../../../test/engine/unit/metadata_adversarial/recovery.rs"]
mod metadata_adversarial_recovery;
