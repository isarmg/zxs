//! Typed, read-only preview of an uninitialized current SMB directory source.
use data_serialization::{Deserialize, Serialize};
use name_policy::{LookupPath, PortablePath};
use storage_contract::{BranchId, ContentHash, EntryId, FileId, RepositoryId, VersionId};

#[derive(Clone, Debug, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadDirectorySnapshotSource {
    pub branch_id: BranchId,
    pub path: Option<LookupPath>,
}

#[derive(Serialize)]
pub struct DirectorySnapshotSourcePreview {
    pub(crate) repository_id: RepositoryId,
    pub(crate) branch_id: BranchId,
    pub(crate) root: Option<PortablePath>,
    pub(crate) initialization: Initialization,
    pub(crate) can_commit: bool,
    pub(crate) physical_initialization_checked: bool,
    pub(crate) full_content_verified: bool,
    pub(crate) entries: Vec<DirectorySnapshotSourceEntry>,
    pub(crate) usage: DirectorySnapshotSourceUsage,
}

#[derive(Serialize)]
#[serialization(rename_all = "snake_case")]
pub(crate) enum Initialization {
    Uninitialized,
}

#[derive(Serialize)]
pub(crate) struct DirectorySnapshotSourceEntry {
    pub(crate) path: PortablePath,
    pub(crate) entry_id: EntryId,
    #[serialization(flatten)]
    pub(crate) content: DirectorySnapshotSourceContent,
}

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DirectorySnapshotSourceContent {
    Directory,
    File {
        file_id: FileId,
        version_id: VersionId,
        #[serialization(with = "storage_contract::u64_string")]
        size: u64,
        content_hash: ContentHash,
    },
}

#[derive(Serialize)]
pub(crate) struct DirectorySnapshotSourceUsage {
    pub(crate) entries: usize,
    pub(crate) path_bytes: usize,
    pub(crate) manifest_bytes: usize,
    pub(crate) unique_versions: usize,
    pub(crate) route_references: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) logical_bytes: u64,
    pub(crate) proof_bytes: usize,
    pub(crate) scanned_entries: usize,
    pub(crate) work_units: usize,
}
