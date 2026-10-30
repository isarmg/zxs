//! Shared namespace rules and domain operations.
use crate::*;
use std::time::{SystemTime, UNIX_EPOCH};
mod ancestry;
mod batch;
pub(crate) mod directory;
mod file;
pub(crate) use file::pending_file_source;
pub mod file_history_pages;
pub(crate) mod file_merges;
mod file_metadata;
pub(crate) mod file_nodes;
mod file_paths;
pub(crate) mod file_publication;
mod file_requests;
mod file_tags;
mod history;
pub(crate) use history::tree_commit;
pub mod history_graph_pages;
mod namespace;
mod repository;
mod workspace;
pub(crate) use ancestry::descendant_commits;
pub use file_nodes::{FileNodeProjectionRequest, FileNodeSummary};
pub use file_requests::{FileBranchMergeRequest, FileNodeTagsUpdateRequest};
pub use namespace::NamespaceTree;
#[cfg(test)]
pub(crate) use namespace::find_unordered;
pub(crate) use namespace::{
    apply_workspace_namespace, deletion_record, ensure_parents, find_unordered_key, sorted,
    validate_entries, validate_workspace_dependencies,
};
pub use workspace::{PreparedUpload, UploadPreparation};
pub(crate) use workspace::{VersionBuild, WorkspaceMutation, native_commit_digest};

/// Authorship shared by every record produced by one business operation.
#[derive(Clone, Copy)]
pub(crate) struct OperationContext<'a> {
    principal: &'a str,
    operation_id: OperationId,
}

/// Published namespace objects and the unpublished write-set for one mutation.
pub(crate) struct NamespaceMutation<'a> {
    base: &'a dyn crate::namespace::NamespaceSource,
    fresh: &'a mut crate::namespace::NamespaceObjects,
}

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
