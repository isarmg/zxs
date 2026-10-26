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
pub(crate) struct VersionResponse {
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
    content_state: storage_contract::ContentState,
}
impl From<VersionSnapshot> for VersionResponse {
    fn from(snapshot: VersionSnapshot) -> Self {
        let VersionSnapshot {
            version,
            content_state,
        } = snapshot;
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
