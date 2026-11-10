//! Shared namespace rules and domain operations.
use crate::*;
use std::time::{SystemTime, UNIX_EPOCH};
mod ancestry;
mod batch;
mod directory;
pub(crate) mod directory_git;
mod directory_preview;
pub(crate) mod directory_staging;
pub use directory_git::DirectoryGitStatusRead;
pub use directory_preview::{DirectoryBranchPreview, DirectoryDefaultSelection};
pub use directory_staging::DirectoryCommitRequest;
mod file;
pub(crate) mod file_relocation;
pub(crate) mod history_copy;
pub use history_copy::FileHistoryCopyRequest;
mod history_move;
pub use history_move::{
    CurrentFileHistoryMove, CurrentFileHistoryMoveProjection, FileHistoryMoveCapture,
    FileHistoryMoveProjection, FileHistoryMoveReplacement, FileHistoryMoveRequest,
    FileHistoryMoveSourceDigest,
};
pub(crate) use history_move::{
    FileHistoryMoveReplay, NATIVE_KIND as FILE_HISTORY_MOVE_KIND, NativeMoveCandidate,
    NativeMoveDraft, admit_deletions as admit_file_history_move_deletions,
    admit_displaced as admit_file_history_move_displaced, original_move_intent,
    reject_unclosed_publication as reject_unclosed_file_history_move, replay_selected_move,
    retained_deletion as retained_file_history_move_deletion, verify_current_move_transition,
};
pub(crate) mod file_ancestry;
mod file_history_merges;
pub(crate) mod file_merges;
mod file_metadata;
pub(crate) mod file_nodes;
mod file_paths;
pub(crate) mod file_publication;
mod file_requests;
mod file_tags;
mod history;
mod namespace;
mod repository;
mod workspace;
pub(crate) use ancestry::descendant_commits;
pub(crate) use batch::{relocate as relocate_namespace_entry, separated_file_source};
pub(crate) use directory::context_key_from_key;
pub(crate) use directory::namespace_location;
pub(crate) use directory::record_restored_namespace_with_ids;
pub use file_nodes::{FileNodeProjectionRequest, FileNodeSummary};
pub use file_requests::{
    FileBranchMergeRequest, FileDefaultSelection, FileHistoryMergeRequest,
    FileNodeTagsUpdateRequest,
};
pub use namespace::{CurrentObject, NamespaceTree};
pub(crate) use namespace::{
    apply_namespace, apply_workspace_namespace, deletion_record, ensure_parents, find_unordered,
    find_unordered_key, sorted, validate_entries, validate_workspace_dependencies,
};
pub use workspace::{PreparedUpload, UploadPreparation};
pub(crate) use workspace::{VersionBuild, WorkspaceMutation, native_commit_digest};

pub(crate) fn now() -> Result<u64> {
    unix_seconds(SystemTime::now())
}

fn unix_seconds(time: SystemTime) -> Result<u64> {
    Ok(time.duration_since(UNIX_EPOCH)?.as_secs())
}

pub fn validate_branch_name(name: &str) -> Result<()> {
    storage_contract::validate_branch_name(name).map_err(Into::into)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/composition_tests.rs"]
mod composition_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_paths_tests.rs"]
mod file_paths_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/reference_corruption_tests.rs"]
mod reference_corruption_tests;
