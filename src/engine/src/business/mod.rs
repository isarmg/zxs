//! Shared namespace rules and domain operations.
use crate::*;
use std::time::{SystemTime, UNIX_EPOCH};
mod ancestry;
mod batch;
mod directory;
mod file;
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
pub use file_nodes::{FileNodeProjectionRequest, FileNodeSummary};
pub use file_requests::{FileBranchMergeRequest, FileLocation, FileNodeTagsUpdateRequest};
pub use namespace::NamespaceTree;
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
    if name.len() > 128 {
        return Err(Error::new(
            storage_contract::ErrorCode::MetadataTooLong,
            "branch names must not exceed 128 UTF-8 bytes",
        ));
    }
    name_policy::validate_name(name).map_err(Into::into)
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
