//! Typed, request-local namespace reads. None of these results grants future admission.
use data_serialization::Serialize;

/// A directory handle names either the synthetic root or a canonical directory.
pub struct ReadDirectory<'a> {
    pub branch: &'a str,
    pub entry_id: &'a str,
    pub path: Option<&'a name_policy::PortablePath>,
    pub selection: DirectorySelection<'a>,
}

pub enum DirectorySelection<'a> {
    Capture,
    Page {
        commit: Option<&'a str>,
        after: Option<&'a str>,
    },
}

/// A bounded page; `more` implies at least one entry. Capture returns an empty page.
#[derive(Debug, Serialize)]
pub struct DirectorySnapshot {
    pub commit_id: String,
    pub entries: Vec<storage_contract::Entry>,
    pub more: bool,
}

/// Immutable metadata and its stored disposition from one authorized state.
#[derive(Clone, Debug)]
pub struct VersionSnapshot {
    pub version: storage_contract::FileVersion,
    pub content_state: storage_contract::ContentState,
}

#[derive(Serialize)]
pub(crate) struct VersionRecordResponse {
    history_tree_id: String,
    version_id: String,
    file_id: String,
    creation_primary_slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    logical_size: u64,
    #[serialization(with = "storage_contract::u64_string")]
    version_sequence: u64,
    content_hash: String,
    checksum_chunk_bytes: u32,
    checksum_manifest_hash: String,
    creation_name: String,
    storage_name: String,
    created_at: String,
    source_version_ids: Vec<String>,
    created_as_full_checkpoint: bool,
}
impl From<storage_contract::FileVersion> for VersionRecordResponse {
    fn from(version: storage_contract::FileVersion) -> Self {
        let storage_contract::FileVersion {
            history_tree_id,
            version_id,
            file_id,
            creation_primary_slot_id,
            logical_size,
            version_sequence,
            content_hash,
            checksum_chunk_bytes,
            checksum_manifest_hash,
            creation_name,
            storage_name,
            created_at,
            source_version_ids,
            created_as_full_checkpoint,
        } = version;
        Self {
            history_tree_id,
            version_id,
            file_id,
            creation_primary_slot_id,
            logical_size,
            version_sequence,
            content_hash: content_hash.to_string(),
            checksum_chunk_bytes,
            checksum_manifest_hash,
            creation_name,
            storage_name,
            created_at,
            source_version_ids,
            created_as_full_checkpoint,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct VersionResponse {
    #[serialization(flatten)]
    version: VersionRecordResponse,
    content_state: storage_contract::ContentState,
}
impl From<VersionSnapshot> for VersionResponse {
    fn from(snapshot: VersionSnapshot) -> Self {
        let VersionSnapshot {
            version,
            content_state,
        } = snapshot;
        Self {
            version: version.into(),
            content_state,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_read.rs"]
mod tests;

#[derive(Serialize)]
pub(crate) struct RepositoryResponse {
    repository_id: String,
    name: String,
    history_tree_id: String,
    main_branch_id: String,
    primary_slot_id: String,
    target_copies: u32,
    name_policy: String,
    unicode_version: String,
    smb_enabled: bool,
    #[serialization(with = "storage_contract::u64_string")]
    smb_binding_generation: u64,
    #[serialization(skip_serializing_if = "repository_root_is_false")]
    file_manager_root: bool,
}
fn repository_root_is_false(value: &bool) -> bool {
    !*value
}
impl From<storage_contract::Repository> for RepositoryResponse {
    fn from(repository: storage_contract::Repository) -> Self {
        let storage_contract::Repository {
            repository_id,
            name,
            history_tree_id,
            main_branch_id,
            primary_slot_id,
            target_copies,
            name_policy,
            unicode_version,
            smb_enabled,
            smb_binding_generation,
            file_manager_root,
        } = repository;
        Self {
            repository_id,
            name,
            history_tree_id,
            main_branch_id,
            primary_slot_id,
            target_copies,
            name_policy,
            unicode_version,
            smb_enabled,
            smb_binding_generation,
            file_manager_root,
        }
    }
}

/// A validated existing logical path used only to choose this read's filename.
#[derive(Clone, Debug)]
pub struct DownloadContext {
    pub repository_id: String,
    pub path: name_policy::LookupPath,
}
pub struct ReadContent<'a> {
    pub version: &'a str,
    pub context: Option<&'a DownloadContext>,
}

/// Select a branch head or one immutable commit in the same history.
pub struct ReadTree<'a> {
    pub branch: &'a str,
    pub commit: Option<&'a str>,
}
/// Owned response projection captured during one authorized admission.
#[derive(Debug, Serialize)]
pub struct TreeSnapshot {
    pub branch_id: String,
    pub commit_id: String,
    pub entries: Vec<storage_contract::Entry>,
    pub pending_file_versions: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize)]
pub(crate) struct RepositoryListResponse {
    pub repositories: Vec<RepositoryResponse>,
}

pub struct ResolveEntry<'a> {
    pub branch: &'a str,
    pub path: &'a name_policy::LookupPath,
    pub commit: Option<&'a str>,
}
#[derive(Clone, Debug)]
pub struct ResolvedEntry {
    pub branch_id: String,
    pub commit_id: String,
    pub entry: storage_contract::Entry,
    pub version: Option<storage_contract::FileVersion>,
    pub content: Option<crate::ContentDispositionSnapshot>,
}
#[derive(Serialize)]
pub(crate) struct ResolveResponse {
    branch_id: String,
    commit_id: String,
    entry: storage_contract::Entry,
    version: Option<VersionRecordResponse>,
    #[serialization(flatten)]
    content: Option<crate::backend_branch::ContentDispositionResponse>,
}
impl From<ResolvedEntry> for ResolveResponse {
    fn from(resolved: ResolvedEntry) -> Self {
        let ResolvedEntry {
            branch_id,
            commit_id,
            entry,
            version,
            content,
        } = resolved;
        Self {
            branch_id,
            commit_id,
            entry,
            version: version.map(VersionRecordResponse::from),
            content: content.map(crate::backend_branch::ContentDispositionResponse::from),
        }
    }
}

#[derive(data_serialization::Serialize)]
pub(crate) struct FileHistoryResponse {
    file_id: String,
    versions: Vec<VersionRecordResponse>,
}
impl FileHistoryResponse {
    pub(crate) fn new(file_id: String, versions: Vec<storage_contract::FileVersion>) -> Self {
        Self {
            file_id,
            versions: versions
                .into_iter()
                .map(VersionRecordResponse::from)
                .collect(),
        }
    }
}
