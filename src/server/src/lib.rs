//! zxs HTTP boundary. Storage semantics stay in the synchronous core backend.
pub mod auth;
#[cfg(feature = "core")]
pub mod engine_backend;
mod file_manager;
mod http_auth;
mod http_boundary;
mod http_shutdown;
mod http_spool;
#[cfg(feature = "core")]
pub mod management;
mod service_lifecycle;
mod shutdown;
pub use shutdown::ShutdownSignal;
#[cfg(feature = "core")]
pub mod setup_recovery;
pub use http_boundary::ApiError;
pub mod smb_rpc;
mod spool_budget;
mod telemetry;
pub use spool_budget::SpoolLimits;
mod transfer;
mod transfer_limits;
pub use transfer_limits::TransferLimits;
mod uploads;
mod web_assets;

mod backend_action;
mod backend_smb_binding;
pub use backend_smb_binding::{SmbBindingStatus, UpdateSmbBinding};
mod http_smb_binding;
use http_smb_binding::{smb_binding, update_smb_binding};
mod backend_comparison;
pub use backend_comparison::CompareCommits;
mod http_comparison;
use http_comparison::compare;
mod backend_protection;
pub use backend_protection::{UpdatePlacementRule, UpdateProtectionPolicy};
mod http_protection;
use http_protection::{placement_rule, policy, update_placement_rule, update_policy};
mod backend_storage_metadata;
pub use backend_storage_metadata::{TopologySlot, TopologySnapshot};
mod backend_events;
pub use backend_events::EventPage;
mod backend_graph;
pub use backend_graph::HistoryGraphSnapshot;
mod backend_branch;
pub use backend_branch::{BranchSnapshot, ContentDispositionSnapshot, FileBranchSnapshot};
mod backend_diagnostics;
mod backend_monitoring;
pub use backend_monitoring::{DiskSample, PowerUnavailable};
mod backend_read;
mod backend_status;
pub use backend_status::{Capabilities, EngineReadiness, Readiness, StorageState, StorageStatus};
mod backend_workspace;
pub use backend_workspace::{
    CreateWorkspace, PublishedUploadState, PublishedUploadStatus, StageNamespace,
    WorkspaceWriteSession,
};
mod http_workspace;
pub use backend_read::{
    DirectorySelection, DirectorySnapshot, DownloadContext, ReadContent, ReadDirectory, ReadTree,
    ResolveEntry, ResolvedEntry, TreeSnapshot, VersionSnapshot,
};
use http_workspace::{abort, commit, namespace, seal, workspace, workspace_info, write_session};
mod http_read;
use http_read::resolve;
mod commit_wire;
mod http_operation;
mod operation_wire;
pub use backend_action::BackendAction;
use http_operation::operation;

use auth::{AuthStore, Identity};
use http_auth::{authorize, change_password, login, logout, me};
use http_boundary::{ApiPath, ApiQuery};
use http_routing::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, patch, post, put},
};
use json_codec::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use transfer::{checksums, content, upload, upload_status, workspace_content};

fn upload_fs_error(error: platform_io::fs_safety::Error) -> ApiError {
    ApiError::new(
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::UploadStateInvalid,
        "local upload staging is unsafe or changed",
    )
    .with_source(error)
}

/// A held upload inode and the byte identity verified by the transport.
/// Publication must check this identity before accepting the staged generation.
pub struct UploadSource {
    pub file: std::fs::File,
    pub size: u64,
    pub hash: storage_contract::ContentHash,
}
impl UploadSource {
    pub fn from_file(file: std::fs::File, maximum: u64) -> Result<Self, ApiError> {
        use std::os::unix::fs::FileExt;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&file)
            .map_err(crate::upload_fs_error)?;
        let mut digest = blake3::Hasher::new();
        let mut size = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read_at(&mut buffer, size).map_err(ApiError::io)?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or_else(|| ApiError::limit("upload length overflow"))?;
            if size > maximum {
                return Err(ApiError::limit("upload exceeds byte limit"));
            }
            digest.update(&buffer[..count]);
        }
        Ok(Self {
            file,
            size,
            hash: storage_contract::ContentHash::from_digest(digest.finalize()),
        })
    }
}

/// A successful content open must hold its core read lease until the body drops.
pub struct Content {
    pub fallback_files: Vec<std::fs::File>,
    pub checksum_manifest: Option<storage_contract::ChecksumManifest>,
    pub file: std::fs::File,
    pub size: u64,
    pub hash: String,
    pub lease: Box<dyn Send + Sync>,
    pub download_name: Option<String>,
}
impl Content {
    /// Raw descriptor consumers require a fully verified replica before handoff.
    /// Range consumers instead verify each emitted chunk from the same buffer.
    pub fn fully_verified(mut self) -> Result<Self, ApiError> {
        let Some(manifest) = self.checksum_manifest.as_ref() else {
            return Ok(self);
        };
        loop {
            if verify_content_file(&mut self.file, manifest, self.size, &self.hash).is_ok() {
                self.checksum_manifest = None;
                self.fallback_files.clear();
                return Ok(self);
            }
            let Some(next) = self.fallback_files.pop() else {
                return Err(ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::ContentUnavailable,
                    "no fully verified content replica is available",
                ));
            };
            self.file = next;
        }
    }
}

fn verify_content_file(
    file: &mut std::fs::File,
    manifest: &storage_contract::ChecksumManifest,
    size: u64,
    hash: &str,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let corrupt = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "content verification failed",
        )
    };
    if !storage_contract::SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&manifest.chunk_bytes)
        || manifest.logical_size != size
        || file.metadata()?.len() != size
        || manifest.chunks.len() as u64 != size.div_ceil(manifest.chunk_bytes as u64)
    {
        return Err(corrupt());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut whole = blake3::Hasher::new();
    let mut remaining = size;
    let mut buffer = vec![0; manifest.chunk_bytes as usize];
    for expected in &manifest.chunks {
        let length = remaining.min(manifest.chunk_bytes as u64) as usize;
        file.read_exact(&mut buffer[..length])?;
        if blake3::hash(&buffer[..length]).as_bytes() != expected {
            return Err(corrupt());
        }
        whole.update(&buffer[..length]);
        remaining -= length as u64;
    }
    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0
        || file.metadata()?.len() != size
        || whole.finalize().to_hex().as_str() != hash
    {
        return Err(corrupt());
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}

/// Capability for the service's private, read-only disk sampler.
///
/// External adapters may implement the observation hook, but cannot construct
/// this request. It is not a user identity and has no public dispatch action.
pub struct InternalDiskObservation(());

#[derive(Debug, Clone)]
pub struct CommitWorkspace {
    pub workspace_id: String,
    pub operation_id: storage_contract::OperationId,
    pub expected_head: String,
    pub fencing_token: u64,
    pub message: String,
    pub request_digest: Option<String>,
}

pub trait Backend: Send + Sync {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError>;
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError>;

    fn setup_operation(
        &self,
        _action: &'static str,
        _input: Value,
        _bootstrap: Option<&str>,
        _identity: Option<&Identity>,
    ) -> Result<Value, ApiError> {
        Err(ApiError::unsupported())
    }

    /// Called after system authentication has been placed in shared state.
    fn start_management(&self, _auth: Arc<Mutex<AuthStore>>) {}
    /// Close background recovery admission without waiting for disk I/O.
    fn stop_management(&self) {}
    /// Join owned native recovery writers; called only on a blocking thread.
    fn drain_management(&self) -> Result<(), ApiError> {
        Ok(())
    }
    fn management_status(&self) -> StorageStatus {
        StorageStatus::EmbeddedReady
    }
    fn retry_storage(&self, _identity: &Identity) -> Result<StorageStatus, ApiError> {
        Err(ApiError::unsupported())
    }

    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError>;
    /// Share a frozen published commit after current branch and identity admission.
    /// The returned snapshot owns no authorization for later reads or writes.
    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError>;
    /// Create a workspace using the current authenticated actor and pool admission limits.
    fn create_workspace(
        &self,
        identity: &Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, ApiError>;
    /// Abort only a currently owned workspace; retain existing pending-operation decisions.
    fn abort_workspace(&self, identity: &Identity, workspace: &str) -> Result<(), ApiError>;
    fn seal_workspace(
        &self,
        identity: &Identity,
        workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, ApiError>;
    fn commit_workspace_public(
        &self,
        identity: &Identity,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, ApiError>;
    fn version_snapshot(
        &self,
        identity: &Identity,
        version: &str,
    ) -> Result<VersionSnapshot, ApiError>;
    fn authorize_version(&self, identity: &Identity, version: &str) -> Result<(), ApiError>;
    fn repository_snapshot(
        &self,
        identity: &Identity,
        repository: &str,
    ) -> Result<storage_contract::Repository, ApiError>;
    fn authorize_repository(&self, identity: &Identity, repository: &str) -> Result<(), ApiError>;
    fn capabilities(&self) -> Result<Capabilities, ApiError>;
    fn ready(&self) -> Result<Readiness, ApiError>;
    fn workspace_capacity(
        &self,
        identity: &Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, ApiError>;
    fn smb_binding_status(
        &self,
        identity: &Identity,
        repository: &str,
    ) -> Result<SmbBindingStatus, ApiError>;
    fn update_smb_binding(
        &self,
        identity: &Identity,
        request: UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, ApiError>;
    fn protection_policy(
        &self,
        identity: &Identity,
        id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, ApiError>;
    fn placement_rule(
        &self,
        identity: &Identity,
        id: &str,
    ) -> Result<storage_contract::PlacementRule, ApiError>;
    fn update_protection_policy(
        &self,
        identity: &Identity,
        request: UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, ApiError>;
    fn update_placement_rule(
        &self,
        identity: &Identity,
        request: UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, ApiError>;
    fn compare_commits(
        &self,
        identity: &Identity,
        request: CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, ApiError>;
    fn disk_status(
        &self,
        identity: &Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, ApiError>;
    fn integrity_check(
        &self,
        identity: &Identity,
    ) -> Result<storage_contract::CheckReport, ApiError>;
    fn file_history(
        &self,
        identity: &Identity,
        file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, ApiError>;
    fn topology_snapshot(&self, identity: &Identity) -> Result<TopologySnapshot, ApiError>;
    fn placement_snapshot(
        &self,
        identity: &Identity,
        version: &str,
    ) -> Result<storage_contract::Placement, ApiError>;
    fn event_page(&self, identity: &Identity, after: u64) -> Result<EventPage, ApiError>;
    fn history_graph(
        &self,
        identity: &Identity,
        history: &str,
    ) -> Result<HistoryGraphSnapshot, ApiError>;
    fn commit_snapshot(
        &self,
        identity: &Identity,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError>;
    fn resolve_entry(
        &self,
        identity: &Identity,
        request: ResolveEntry<'_>,
    ) -> Result<ResolvedEntry, ApiError>;
    fn branch_snapshot(
        &self,
        identity: &Identity,
        branch: &str,
    ) -> Result<BranchSnapshot, ApiError>;
    fn branches_snapshot(
        &self,
        identity: &Identity,
        repository: &str,
    ) -> Result<Vec<BranchSnapshot>, ApiError>;
    fn repositories_snapshot(
        &self,
        identity: &Identity,
    ) -> Result<Vec<storage_contract::Repository>, ApiError>;
    fn tree_snapshot(
        &self,
        identity: &Identity,
        request: ReadTree<'_>,
    ) -> Result<TreeSnapshot, ApiError>;
    fn create_write_session(
        &self,
        identity: &Identity,
        branch: &str,
    ) -> Result<WorkspaceWriteSession, ApiError>;
    fn stage_namespace(
        &self,
        identity: &Identity,
        command: StageNamespace,
    ) -> Result<storage_contract::Workspace, ApiError>;
    fn workspace(
        &self,
        identity: &Identity,
        workspace: &str,
    ) -> Result<storage_contract::Workspace, ApiError>;
    /// None means an absent item in an authorized workspace, never failed admission.
    fn published_upload_status(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Option<PublishedUploadStatus>, ApiError>;
    /// Revalidate ownership, workspace lease and declared file/workspace size limits.
    fn authorize_upload(
        &self,
        identity: &Identity,
        workspace: &str,
        logical_size: Option<u64>,
    ) -> Result<(), ApiError>;
    /// Capture or page a directory after current admission; enforce the page byte budget.
    fn read_directory(
        &self,
        identity: &Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, ApiError>;
    /// Check the captured write destination against current selection and ownership.
    fn authorize_workspace_selection(
        &self,
        identity: &Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), ApiError>;
    /// Revalidate current branch admission without capturing namespace entries.
    fn authorize_current(&self, identity: &Identity, branch: &str) -> Result<(), ApiError>;
    /// Look up one current entry after live identity and branch admission.
    fn lookup_current(
        &self,
        identity: &Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError>;
    /// Look up one entry in a frozen published snapshot, with current admission.
    /// A missing path is `Ok(None)`; admission and snapshot failures are errors.
    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError>;
    /// Consume the held inode with positional reads and verify its declared
    /// size/hash against the copied snapshot before accepting any generation.
    /// Publish the next workspace generation chosen under current admission.
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError>;
    /// Return a fully verified raw descriptor (used by SMB and local consumers).
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError>;
    /// HTTP may authenticate only requested chunks, while retaining the read lease.
    fn range_content(
        &self,
        identity: &Identity,
        request: ReadContent<'_>,
    ) -> Result<Content, ApiError>;
    /// Publish the exact caller generation; adapters must not silently discard it.
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<storage_contract::StagedFile, ApiError>;
    fn workspace_content(
        &self,
        _identity: &Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Content, ApiError> {
        Err(ApiError::unsupported())
    }
    fn checksums(&self, _identity: &Identity, _version: &str) -> Result<Vec<u8>, ApiError> {
        Err(ApiError::unsupported())
    }
    /// Internal monitoring only. Protocol requests must use authenticated
    /// actions instead; support is opt-in and never derived from an identity.
    fn observe_disks(
        &self,
        _: &InternalDiskObservation,
    ) -> Result<Vec<crate::DiskSample>, ApiError> {
        Err(ApiError::unsupported())
    }
}

/// Re-check current administrator credentials even for trusted local protocol adapters.
struct AuthorizedBackend {
    backend: Arc<dyn Backend>,
    auth: auth::AuthReader,
}
impl AuthorizedBackend {
    fn current(&self, identity: &Identity) -> Result<Identity, ApiError> {
        self.auth.revalidate_identity(identity)
    }
}
impl Backend for AuthorizedBackend {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.backend
            .operation(&self.current(identity)?, operation_id)
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.backend
            .commit_workspace(&self.current(identity)?, command)
    }

    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError> {
        self.backend
            .snapshot_at(&self.current(identity)?, branch, commit)
    }

    fn create_workspace(
        &self,
        identity: &Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, ApiError> {
        self.backend
            .create_workspace(&self.current(identity)?, command)
    }
    fn abort_workspace(&self, identity: &Identity, workspace: &str) -> Result<(), ApiError> {
        self.backend
            .abort_workspace(&self.current(identity)?, workspace)
    }

    fn workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.backend.workspace(&self.current(identity)?, workspace)
    }
    fn published_upload_status(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        self.backend
            .published_upload_status(&self.current(identity)?, workspace, item)
    }
    fn seal_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        self.backend
            .seal_workspace(&self.current(identity)?, workspace)
    }
    fn commit_workspace_public(
        &self,
        identity: &crate::auth::Identity,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.backend
            .commit_workspace_public(&self.current(identity)?, command)
    }
    fn version_snapshot(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        self.backend
            .version_snapshot(&self.current(identity)?, version)
    }
    fn authorize_version(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<(), crate::ApiError> {
        self.backend
            .authorize_version(&self.current(identity)?, version)
    }
    fn repository_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.backend
            .repository_snapshot(&self.current(identity)?, id)
    }
    fn authorize_repository(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<(), crate::ApiError> {
        self.backend
            .authorize_repository(&self.current(identity)?, id)
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        self.backend.capabilities()
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        self.backend.ready()
    }
    fn workspace_capacity(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        self.backend.workspace_capacity(&self.current(identity)?)
    }
    fn smb_binding_status(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        self.backend
            .smb_binding_status(&self.current(identity)?, id)
    }
    fn update_smb_binding(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.backend
            .update_smb_binding(&self.current(identity)?, request)
    }
    fn protection_policy(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        self.backend.protection_policy(&self.current(identity)?, id)
    }
    fn placement_rule(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        self.backend.placement_rule(&self.current(identity)?, id)
    }
    fn update_protection_policy(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.backend
            .update_protection_policy(&self.current(identity)?, request)
    }
    fn update_placement_rule(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.backend
            .update_placement_rule(&self.current(identity)?, request)
    }
    fn compare_commits(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        self.backend
            .compare_commits(&self.current(identity)?, request)
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        self.backend.disk_status(&self.current(identity)?)
    }
    fn integrity_check(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        self.backend.integrity_check(&self.current(identity)?)
    }
    fn file_history(
        &self,
        identity: &crate::auth::Identity,
        file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        self.backend.file_history(&self.current(identity)?, file)
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        self.backend.topology_snapshot(&self.current(identity)?)
    }
    fn placement_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        self.backend
            .placement_snapshot(&self.current(identity)?, id)
    }
    fn event_page(
        &self,
        identity: &crate::auth::Identity,
        after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        self.backend.event_page(&self.current(identity)?, after)
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        self.backend.history_graph(&self.current(identity)?, id)
    }
    fn commit_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.backend.commit_snapshot(&self.current(identity)?, id)
    }
    fn resolve_entry(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        self.backend
            .resolve_entry(&self.current(identity)?, request)
    }
    fn branch_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.backend.branch_snapshot(&self.current(identity)?, id)
    }
    fn branches_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        self.backend.branches_snapshot(&self.current(identity)?, id)
    }
    fn repositories_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        self.backend.repositories_snapshot(&self.current(identity)?)
    }
    fn tree_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.backend
            .tree_snapshot(&self.current(identity)?, request)
    }
    fn create_write_session(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        self.backend
            .create_write_session(&self.current(identity)?, branch)
    }
    fn stage_namespace(
        &self,
        identity: &crate::auth::Identity,
        command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.backend
            .stage_namespace(&self.current(identity)?, command)
    }
    fn authorize_upload(
        &self,
        identity: &Identity,
        workspace: &str,
        logical_size: Option<u64>,
    ) -> Result<(), ApiError> {
        self.backend
            .authorize_upload(&self.current(identity)?, workspace, logical_size)
    }

    fn read_directory(
        &self,
        identity: &Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, ApiError> {
        self.backend
            .read_directory(&self.current(identity)?, request)
    }

    fn authorize_workspace_selection(
        &self,
        identity: &Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), ApiError> {
        self.backend
            .authorize_workspace_selection(&self.current(identity)?, workspace, path)
    }

    fn authorize_current(&self, identity: &Identity, branch: &str) -> Result<(), ApiError> {
        self.backend
            .authorize_current(&self.current(identity)?, branch)
    }

    fn lookup_current(
        &self,
        identity: &Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        self.backend
            .lookup_current(&self.current(identity)?, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        self.backend
            .lookup_at(&self.current(identity)?, branch, commit, path)
    }

    // Startup runs on the raw backend before this wrapper is constructed;
    // telemetry also retains that raw backend for private disk sampling. Keep
    // start_management and observe_disks on their inert defaults here.
    fn stop_management(&self) {
        self.backend.stop_management();
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        self.backend.drain_management()
    }
    fn setup_operation(
        &self,
        action: &'static str,
        input: Value,
        bootstrap: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<Value, ApiError> {
        let current = identity
            .map(|identity| self.current(identity))
            .transpose()?;
        self.backend
            .setup_operation(action, input, bootstrap, current.as_ref())
    }

    fn management_status(&self) -> StorageStatus {
        self.backend.management_status()
    }
    fn retry_storage(&self, identity: &Identity) -> Result<StorageStatus, ApiError> {
        self.backend.retry_storage(&self.current(identity)?)
    }

    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        self.backend.call(action, &self.current(identity)?, input)
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.backend
            .upload(&self.current(identity)?, workspace, item, path, source)
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.backend.upload_generation(
            &self.current(identity)?,
            workspace,
            item,
            path,
            source,
            generation,
        )
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.backend.content(&self.current(identity)?, version)
    }
    fn range_content(
        &self,
        identity: &Identity,
        request: ReadContent<'_>,
    ) -> Result<Content, ApiError> {
        self.backend
            .range_content(&self.current(identity)?, request)
    }
    fn workspace_content(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Content, ApiError> {
        self.backend
            .workspace_content(&self.current(identity)?, workspace, item)
    }
    fn checksums(&self, identity: &Identity, version: &str) -> Result<Vec<u8>, ApiError> {
        self.backend.checksums(&self.current(identity)?, version)
    }
}

#[derive(Clone)]
pub struct SmbConfig {
    pub socket: PathBuf,
    /// Durable system-disk storage, independent of the disposable socket directory.
    pub spool_dir: PathBuf,
    pub allowed_uid: u32,
    pub spool_limits: SpoolLimits,
    /// The single non-guest account authenticated by Samba.
    pub username: String,
}
#[derive(Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    /// Required when TLS terminates upstream. Origin comparison never uses forwarded headers.
    pub external_origin: Option<String>,
    /// Exact loopback proxy peers allowed to supply a single, overwritten X-Forwarded-For IP.
    pub trusted_proxy_ips: Vec<std::net::IpAddr>,
    pub spool_dir: PathBuf,
    pub web_dir: Option<PathBuf>,
    pub max_upload_bytes: u64,
    /// HTTP spool only; independent of engine workspace and SMB storage.
    pub http_spool_limits: SpoolLimits,
    pub http_transfer_limits: TransferLimits,
    pub smb: Option<SmbConfig>,
}
impl ServerConfig {
    pub fn loopback(root: &Path) -> Self {
        Self {
            listen: "127.0.0.1:7400".parse().unwrap(),
            external_origin: None,
            trusted_proxy_ips: Vec::new(),
            spool_dir: root.join("api-spool"),
            web_dir: None,
            max_upload_bytes: 1024 * 1024 * 1024,
            http_spool_limits: SpoolLimits::default(),
            http_transfer_limits: TransferLimits::default(),
            smb: None,
        }
    }
    fn prepare_spool(&self) -> Result<(), ApiError> {
        platform_io::fs_safety::PrivateDirectory::create_trusted(&self.spool_dir)
            .map_err(crate::upload_fs_error)?;
        Ok(())
    }
    fn open_http_spool(&self) -> Option<Arc<spool_budget::SpoolBudget>> {
        // Configuration validation remains mandatory. Only optional filesystem
        // preparation/inventory is isolated, and no unbudgeted fallback exists.
        match self.prepare_spool().and_then(|()| {
            spool_budget::SpoolBudget::recover(&self.spool_dir, self.http_spool_limits.clone())
        }) {
            Ok(budget) => Some(budget),
            Err(error) => {
                tracing::error!(error = ?error, "HTTP upload spool unavailable; management HTTP remains available");
                None
            }
        }
    }
    fn admit(self) -> Result<Arc<AdmittedServerConfig>, ApiError> {
        if !self.listen.ip().is_loopback() {
            return Err(ApiError::invalid(
                "API must listen on loopback; publish through a TLS terminator",
            ));
        }
        if !self.trusted_proxy_ips.is_empty()
            && (self.external_origin.is_none()
                || self.trusted_proxy_ips.iter().any(|ip| !ip.is_loopback()))
        {
            return Err(ApiError::invalid(
                "trusted proxies must be exact loopback IPs and require an HTTPS external origin",
            ));
        }
        self.http_spool_limits.validate()?;
        self.http_transfer_limits.validate()?;
        if self.max_upload_bytes == 0 {
            return Err(ApiError::invalid("upload limit must be positive"));
        }
        if self
            .smb
            .as_ref()
            .is_some_and(|smb| smb_protocol::protocol::validate_username(&smb.username).is_err())
        {
            return Err(ApiError::invalid(
                "SMB requires one explicit authenticated Samba username",
            ));
        }
        let external_origin = self
            .external_origin
            .as_deref()
            .map(ExternalOrigin::parse)
            .transpose()?;
        Ok(Arc::new(AdmittedServerConfig {
            raw: self,
            external_origin,
        }))
    }
}

/// Immutable startup proof. Only shared borrows of the original configuration
/// are exposed, so its normalized origin cannot become stale.
struct AdmittedServerConfig {
    raw: ServerConfig,
    external_origin: Option<ExternalOrigin>,
}
impl std::ops::Deref for AdmittedServerConfig {
    type Target = ServerConfig;
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}
impl AdmittedServerConfig {
    fn secure_cookie(&self) -> bool {
        self.external_origin.is_some()
    }
}

/// Canonical browser serialization of an admitted HTTPS origin.
struct ExternalOrigin(String);
impl ExternalOrigin {
    fn parse(origin: &str) -> Result<Self, ApiError> {
        let parsed = url_encoding::address::Url::parse(origin).map_err(ApiError::invalid_source)?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || origin.ends_with('/')
        {
            return Err(ApiError::invalid(
                "external origin must be an HTTPS origin without trailing slash",
            ));
        }
        Ok(Self(parsed.origin().ascii_serialization()))
    }
}

// A conservative, per-router bound for general HTTP backend jobs. This is
// independent of content/upload admission and is not a process-wide I/O budget.
const HTTP_BACKEND_SLOTS: usize = 8;

#[derive(Clone)]
struct AppState {
    auth_reader: auth::AuthReader,
    backend: Arc<dyn Backend>,
    auth: Arc<Mutex<AuthStore>>,
    config: Arc<AdmittedServerConfig>,
    attempts: Arc<Mutex<HashMap<String, http_auth::Attempts>>>,
    #[cfg(feature = "core")]
    setup_attempts: Arc<Mutex<HashMap<String, http_auth::Attempts>>>,
    #[cfg(feature = "core")]
    recovery_attempts: Arc<Mutex<HashMap<String, http_auth::Attempts>>>,
    pending_logins: Arc<Mutex<std::collections::HashSet<String>>>,
    password_slots: Arc<tokio::sync::Semaphore>,
    authorization_slots: Arc<tokio::sync::Semaphore>,
    backend_slots: Arc<tokio::sync::Semaphore>,
    management_slots: Arc<tokio::sync::Semaphore>,
    upload_slots: Arc<transfer_limits::Admission>,
    read_slots: Arc<transfer_limits::Admission>,
    upload_claims: Arc<Mutex<std::collections::BTreeSet<String>>>,
    spool_budget: Arc<http_spool::HttpSpool>,
    static_etags: Arc<BTreeMap<String, String>>,
    web_assets_available: bool,
    telemetry: Arc<telemetry::Telemetry>,
}
impl AppState {
    fn http_spool(&self) -> Result<Arc<spool_budget::SpoolBudget>, ApiError> {
        self.spool_budget.budget()
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().expect("service state lock poisoned")
}

/// Construct an embedded router synchronously, including complete HTTP spool
/// inventory. Network serving uses pending staging and inventories in the owned
/// auxiliary startup future instead; neither path admits unaccounted uploads.
///
/// # Errors
/// Invalid configuration (including the external HTTPS origin) is rejected
/// before router startup I/O. Management startup failures are propagated.
pub fn router(
    backend: Arc<dyn Backend>,
    auth: AuthStore,
    config: ServerConfig,
) -> Result<Router, ApiError> {
    let config = config.admit()?;
    let spool_budget = http_spool::HttpSpool::open_blocking(&config);
    let runtime = ManagementRuntime::start(backend, auth, &config)?;
    router_shared(
        runtime.backend,
        runtime.auth,
        config,
        runtime.telemetry,
        spool_budget,
    )
}

/// Common initialization for embedded routers and the network server. Callers
/// validate first; serve binds HTTP before starting background management work.
struct ManagementRuntime {
    backend: Arc<dyn Backend>,
    auth: Arc<Mutex<AuthStore>>,
    telemetry: Arc<telemetry::Telemetry>,
}
impl ManagementRuntime {
    fn start(
        backend: Arc<dyn Backend>,
        auth: AuthStore,
        config: &ServerConfig,
    ) -> Result<Self, ApiError> {
        let telemetry = telemetry::Telemetry::start(
            backend.clone(),
            &config
                .spool_dir
                .parent()
                .ok_or_else(|| ApiError::invalid("spool directory requires a parent"))?
                .join("telemetry-cache"),
        )?;
        let auth = Arc::new(Mutex::new(auth));
        backend.start_management(auth.clone());
        let backend = Arc::new(AuthorizedBackend {
            backend,
            auth: lock(&auth).reader(),
        });
        Ok(Self {
            backend,
            auth,
            telemetry,
        })
    }
}

fn router_shared(
    backend: Arc<dyn Backend>,
    auth: Arc<Mutex<AuthStore>>,
    config: Arc<AdmittedServerConfig>,
    telemetry: Arc<telemetry::Telemetry>,
    spool_budget: Arc<http_spool::HttpSpool>,
) -> Result<Router, ApiError> {
    let static_etags = config.web_dir.as_ref().and_then(|root| {
        match web_assets::static_etags(root) {
            Ok(etags) => Some(etags),
            Err(error) => {
                tracing::error!(error = ?error, "Web assets unavailable; management HTTP APIs remain available");
                None
            }
        }
    });
    let web_assets_available = static_etags.is_some();
    let auth_reader = lock(&auth).reader();
    let state = AppState {
        auth_reader,
        backend,
        auth,
        telemetry,
        config: Arc::clone(&config),
        spool_budget,
        attempts: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(feature = "core")]
        setup_attempts: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(feature = "core")]
        recovery_attempts: Arc::new(Mutex::new(HashMap::new())),
        pending_logins: Arc::new(Mutex::new(std::collections::HashSet::new())),
        password_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        authorization_slots: Arc::new(tokio::sync::Semaphore::new(8)),
        backend_slots: Arc::new(tokio::sync::Semaphore::new(HTTP_BACKEND_SLOTS)),
        management_slots: Arc::new(tokio::sync::Semaphore::new(2)),
        upload_slots: transfer_limits::Admission::new(&config.http_transfer_limits),
        read_slots: transfer_limits::Admission::new(&config.http_transfer_limits),
        upload_claims: Arc::new(Mutex::new(Default::default())),
        static_etags: Arc::new(static_etags.unwrap_or_default()),
        web_assets_available,
    };
    let protected = Router::new();
    #[cfg(feature = "core")]
    let protected = protected.merge(setup_recovery::protected_routes());
    let protected = protected
        .merge(file_manager::routes())
        .route("/api/system/storage", get(storage_status))
        .route("/api/system/storage/retry", post(storage_retry))
        .route("/api/system/status", get(telemetry::status))
        .route("/api/system/history", get(telemetry::history))
        .route("/api/auth/me", get(me))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/password", post(change_password))
        .route(
            "/api/repositories",
            get(repositories).post(create_repository),
        )
        .route("/api/repositories/{repo}", get(repository))
        .route(
            "/api/repositories/{repo}/smb-binding",
            get(smb_binding).put(update_smb_binding),
        )
        .route(
            "/api/repositories/{repo}/default-branch",
            get(default_branch).put(update_default_branch),
        )
        .route(
            "/api/repositories/{repo}/directory-scopes",
            post(initialize_directory_scope),
        )
        .route(
            "/api/repositories/{repo}/directory-branches",
            get(directory_branch_scopes).post(query_directory_branch_scopes),
        )
        .route(
            "/api/repositories/{repo}/directory-branches/{id}",
            patch(rename_directory_branch),
        )
        .route(
            "/api/repositories/{repo}/branches",
            get(branches).post(create_branch),
        )
        .route("/api/branches/{branch}/tree", get(tree))
        .route("/api/branches/{branch}", get(branch_info))
        .route("/api/branches/{branch}/resolve", post(resolve))
        .route("/api/branches/{branch}/write-sessions", post(write_session))
        .route("/api/branches/{branch}/archive", post(archive))
        .route("/api/workspaces", post(workspace))
        .route("/api/workspaces/capacity", get(workspace_capacity))
        .route("/api/workspaces/{workspace}", get(workspace_info))
        .route(
            "/api/workspaces/{workspace}/files/{item}/content",
            put(upload).get(workspace_content).head(workspace_content),
        )
        .route(
            "/api/workspaces/{workspace}/files/{item}/upload-status",
            get(upload_status),
        )
        .route(
            "/api/workspaces/{workspace}/namespace-operations",
            post(namespace),
        )
        .route("/api/workspaces/{workspace}/commit", post(commit))
        .route("/api/workspaces/{workspace}/seal", post(seal))
        .route("/api/workspaces/{workspace}/abort", post(abort))
        .route("/api/operations/{operation}", get(operation))
        .route(
            "/api/versions/{version}/content",
            get(content).head(content),
        )
        .route("/api/versions/{version}", get(version))
        .route("/api/versions/{version}/placement", get(placement))
        .route("/api/versions/{version}/checksums", get(checksums))
        .route("/api/files/{file}/history", get(history))
        .route("/api/commits/{commit}", get(commit_info))
        .route("/api/history-trees/{history}/graph", get(graph))
        .route("/api/history-trees/{history}/merge-links", post(merge_link))
        .route("/api/history-trees/{history}/purge-plans", post(purge_plan))
        .route("/api/history-trees/{history}/split-plans", post(split_plan))
        .route(
            "/api/history-trees/{history}/historical-save-plans",
            post(historical_save_plan),
        )
        .route("/api/plans/{plan}/commit", post(plan_commit))
        .route("/api/plans/{plan}", get(plan_info))
        .route("/api/plans/{plan}/cancel", post(plan_cancel))
        .route("/api/admin/disks", get(disks).post(register_disk))
        .route("/api/admin/check", get(check))
        .route("/api/backup-topologies/default", get(topology))
        .route("/api/policies/{policy}", get(policy).put(update_policy))
        .route(
            "/api/primary-placement-rules/{rule}",
            get(placement_rule).put(update_placement_rule),
        )
        .route("/api/events", get(events))
        .route("/api/compare", post(compare))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
    let app = Router::new();
    #[cfg(feature = "core")]
    let app = app.merge(setup_recovery::public_routes());
    let mut app = app
        .merge(protected)
        .route(
            "/healthz",
            get(|| async { Json(json!({"status":"alive"})) }),
        )
        .route("/readyz", get(ready))
        .route("/api/management/status", get(management_status))
        .route("/api/capabilities", get(capabilities))
        .route("/api/auth/login", post(login))
        .method_not_allowed_fallback(http_boundary::method_not_allowed)
        .fallback(http_boundary::not_found);
    if let Some(web_dir) = &config.web_dir {
        app = app.fallback_service(if web_assets_available {
            web_assets::fallback(web_dir.clone())
        } else {
            web_assets::unavailable_fallback()
        });
    }
    Ok(app
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            web_assets::static_validation,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::new(tokio::sync::Semaphore::new(64)),
            http_boundary::admit_request,
        ))
        .layer(middleware::from_fn(security_headers))
        .with_state(state))
}

#[derive(Debug, error_definition::Error)]
pub enum ServeError {
    #[error("{0}")]
    Api(#[from] ApiError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub async fn serve(
    backend: Arc<dyn Backend>,
    auth: AuthStore,
    config: ServerConfig,
) -> Result<(), ServeError> {
    let shutdown = match ShutdownSignal::install() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            // Embedders may already have started native management work.
            service_lifecycle::ManagementShutdown::new(backend.clone())
                .finish()
                .await?;
            return Err(error.into());
        }
    };
    serve_with_shutdown(backend, auth, config, shutdown).await
}

/// Serve with handlers installed by the process before opening management state.
///
/// # Errors
/// Returns invalid configuration, listener I/O, or lifecycle failures. Static
/// configuration is admitted once before binding or starting auxiliary work.
pub async fn serve_with_shutdown(
    backend: Arc<dyn Backend>,
    auth: AuthStore,
    config: ServerConfig,
    mut shutdown: ShutdownSignal,
) -> Result<(), ServeError> {
    let mut management_shutdown = service_lifecycle::ManagementShutdown::new(backend.clone());
    if shutdown.observed() {
        management_shutdown.finish().await?;
        return Ok(());
    }
    let config = config.admit()?;
    let (spool_budget, spool_startup) = http_spool::HttpSpool::pending();
    // Skip further startup if the runtime has already delivered a stop signal.
    // Signals still awaiting reactor dispatch are handled by shutdown.wait()
    // below; this snapshot is not an atomic cutoff at OS signal arrival.
    if shutdown.observed() {
        management_shutdown.finish().await?;
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let ManagementRuntime {
        backend,
        auth,
        telemetry,
    } = ManagementRuntime::start(backend, auth, &config)?;
    // Keep optional bridge setup isolated from management lifecycle failures.
    let smb = config.smb.clone();
    let smb_max_file_bytes = config.max_upload_bytes;
    let bridge_auth = auth.clone();
    let spool_config = Arc::clone(&config);
    let app = router_shared(backend.clone(), auth, config, telemetry, spool_budget)?;
    let http = |stopped| http_shutdown::serve(listener, app, stopped);
    let smb_connections = management_shutdown.smb_connections();
    let bridge = async move {
        if let Some(smb) = smb {
            let run = async move {
                smb_rpc::serve_owned(
                    smb,
                    bridge_auth,
                    backend,
                    smb_max_file_bytes,
                    smb_connections,
                )
                .await
                .map_err(|source| {
                    ApiError::new(
                        ::http::StatusCode::SERVICE_UNAVAILABLE,
                        storage_contract::ErrorCode::SmbUnavailable,
                        "SMB bridge stopped",
                    )
                    .with_source(source)
                })
            };
            if let Err(error) = run.await {
                tracing::error!(error = ?error, "SMB bridge stopped; management HTTP remains available");
            }
        }
    };
    // One owned auxiliary future: no detached async readiness publisher. On
    // shutdown, dropping it closes SMB and abandons late spool publication;
    // started blocking inventory remains joined by runtime teardown.
    let auxiliary = async move {
        tokio::join!(spool_startup.run(spool_config), bridge);
    };
    service_lifecycle::run_with_management(http, auxiliary, shutdown.wait(), management_shutdown)
        .await?;
    Ok(())
}

async fn security_headers(request: Request, next: Next) -> Response {
    let sensitive = http_boundary::is_api_path(request.uri().path())
        || matches!(request.uri().path(), "/healthz" | "/readyz");
    let mut response = next.run(request).await;
    // Do not weaken an explicit unavailable-capability response's no-store.
    let no_store = sensitive
        || response
            .headers()
            .get(header::CACHE_CONTROL)
            .is_some_and(|value| value == "no-store");
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if no_store { "no-store" } else { "no-cache" }),
    );
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; font-src 'self'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
}

async fn call(
    state: AppState,
    identity: Identity,
    action: crate::BackendAction,
    input: Value,
) -> Result<Json<Value>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        // The job, including time queued in Tokio, owns admission. Dropping the
        // HTTP future must not release capacity while blocking work continues.
        let _permit = permit;
        state.backend.call(action, &identity, input)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn json_body(request: Request) -> Result<Value, ApiError> {
    let bytes = http_boundary::read_body(request, 1024 * 1024).await?;
    if bytes.is_empty() {
        Ok(json!({}))
    } else {
        storage_contract::strict_json(&bytes).map_err(ApiError::invalid_source)
    }
}
fn with_id(mut body: Value, key: &str, id: String) -> Result<Value, ApiError> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid("expected JSON object"))?;
    if object
        .get(key)
        .is_some_and(|value| value.as_str() != Some(&id))
    {
        return Err(ApiError::invalid("path and body identity disagree"));
    }
    object.insert(key.into(), id.into());
    Ok(body)
}

macro_rules! read_handler {
    ($name:ident, $action:expr, $key:literal) => {
        async fn $name(
            State(state): State<AppState>,
            http_routing::Extension(identity): http_routing::Extension<Identity>,
            ApiPath(id): ApiPath<String>,
            ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
        ) -> Result<Json<Value>, ApiError> {
            let mut input = json!({"query": query});
            input[$key] = id.into();
            call(state, identity, $action, input).await
        }
    };
}
macro_rules! post_handler {
    ($name:ident, $action:expr) => {
        async fn $name(
            State(state): State<AppState>,
            http_routing::Extension(identity): http_routing::Extension<Identity>,
            request: Request,
        ) -> Result<Json<Value>, ApiError> {
            call(state, identity, $action, json_body(request).await?).await
        }
    };
}
macro_rules! id_post_handler {
    ($name:ident, $action:expr, $id:literal) => {
        async fn $name(
            State(state): State<AppState>,
            http_routing::Extension(identity): http_routing::Extension<Identity>,
            ApiPath(id): ApiPath<String>,
            request: Request,
        ) -> Result<Json<Value>, ApiError> {
            call(
                state,
                identity,
                $action,
                with_id(json_body(request).await?, $id, id)?,
            )
            .await
        }
    };
}
async fn repositories(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_read::RepositoryListResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .repositories_snapshot(&identity)
            .map(|repositories| backend_read::RepositoryListResponse {
                repositories: repositories
                    .into_iter()
                    .map(backend_read::RepositoryResponse::from)
                    .collect(),
            })
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn disks(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_diagnostics::DisksResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .disk_status(&identity)
            .map(backend_diagnostics::DisksResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn events(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_events::EventPageResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let after = storage_contract::parse_u64_decimal(
            query.get("after").map(String::as_str).unwrap_or("0"),
        )
        .map_err(ApiError::invalid_source)?;
        state
            .backend
            .event_page(&identity, after)
            .map(backend_events::EventPageResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn check(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_diagnostics::CheckResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .integrity_check(&identity)
            .map(backend_diagnostics::CheckResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn repository(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_read::RepositoryResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .repository_snapshot(&identity, &id)
            .map(backend_read::RepositoryResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

read_handler!(
    default_branch,
    crate::BackendAction::DefaultBranch,
    "repository_id"
);

async fn branch_info(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_branch::BranchResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .branch_snapshot(&identity, &id)
            .map(backend_branch::BranchResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn branches(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_branch::BranchListResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .branches_snapshot(&identity, &id)
            .map(|branches| backend_branch::BranchListResponse {
                branches: branches
                    .into_iter()
                    .map(backend_branch::BranchResponse::from)
                    .collect(),
            })
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn tree(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(branch): ApiPath<String>,
    ApiQuery(mut query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<TreeSnapshot>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    let commit = query.remove("commit_id");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state.backend.tree_snapshot(
            &identity,
            ReadTree {
                branch: &branch,
                commit: commit.as_deref(),
            },
        )
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn version(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_read::VersionResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .version_snapshot(&identity, &id)
            .map(backend_read::VersionResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn placement(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(version): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_storage_metadata::PlacementResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .placement_snapshot(&identity, &version)
            .map(backend_storage_metadata::PlacementResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn history(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_read::FileHistoryResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .file_history(&identity, &file)
            .map(|versions| backend_read::FileHistoryResponse::new(file, versions))
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn commit_info(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(commit): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<commit_wire::CommitResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .commit_snapshot(&identity, &commit)
            .map(commit_wire::CommitResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn graph(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(history): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_graph::HistoryGraphResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .history_graph(&identity, &history)
            .map(backend_graph::HistoryGraphResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn topology(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_storage_metadata::TopologyResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .topology_snapshot(&identity)
            .map(backend_storage_metadata::TopologyResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

read_handler!(plan_info, crate::BackendAction::Plan, "plan_id");

read_handler!(
    directory_branch_scopes,
    crate::BackendAction::DirectoryBranchScopes,
    "repository_id"
);
id_post_handler!(
    query_directory_branch_scopes,
    crate::BackendAction::DirectoryBranchSelection,
    "repository_id"
);
post_handler!(create_repository, crate::BackendAction::CreateRepository);
post_handler!(register_disk, crate::BackendAction::RegisterDisk);

id_post_handler!(
    create_branch,
    crate::BackendAction::CreateBranch,
    "repository_id"
);

id_post_handler!(
    initialize_directory_scope,
    crate::BackendAction::InitializeDirectoryScope,
    "repository_id"
);
id_post_handler!(
    update_default_branch,
    crate::BackendAction::UpdateDefaultBranch,
    "repository_id"
);
async fn rename_directory_branch(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((repo, id)): ApiPath<(String, String)>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let input = with_id(json_body(request).await?, "repository_id", repo)?;
    let input = with_id(input, "directory_branch_id", id)?;
    call(
        state,
        identity,
        crate::BackendAction::RenameDirectoryBranch,
        input,
    )
    .await
}
id_post_handler!(archive, crate::BackendAction::Archive, "branch_id");

id_post_handler!(
    merge_link,
    crate::BackendAction::MergeLink,
    "history_tree_id"
);
id_post_handler!(
    purge_plan,
    crate::BackendAction::PurgePlan,
    "history_tree_id"
);
id_post_handler!(
    split_plan,
    crate::BackendAction::SplitPlan,
    "history_tree_id"
);
id_post_handler!(
    historical_save_plan,
    crate::BackendAction::HistoricalSavePlan,
    "history_tree_id"
);

id_post_handler!(plan_commit, crate::BackendAction::PlanCommit, "plan_id");
id_post_handler!(plan_cancel, crate::BackendAction::PlanCancel, "plan_id");

async fn management_status(State(state): State<AppState>) -> Json<Value> {
    // Never wait on password verification or credential publication on a Tokio
    // worker. Contention is temporary and is not evidence of corrupt credentials.
    let authentication = match state.auth.try_lock() {
        Ok(auth) if auth.is_available() => "ready",
        Ok(_) | Err(std::sync::TryLockError::Poisoned(_)) => "recovery_required",
        Err(std::sync::TryLockError::WouldBlock) => "busy",
    };
    Json(
        json!({"management":"alive", "authentication":authentication,
        "storage":backend_status::StorageStatusResponse::from(state.backend.management_status()),
        "auxiliary": {
            "http_uploads": state.spool_budget.status(),
            "web_assets": if state.config.web_dir.is_none() { "not_configured" }
                else if state.web_assets_available { "ready" } else { "unavailable" }
        }}),
    )
}
async fn workspace_capacity(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
) -> Result<Json<storage_contract::WorkspaceCapacity>, ApiError> {
    let permit = state
        .management_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("management capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state.backend.workspace_capacity(&identity)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

async fn storage_status(
    State(state): State<AppState>,
    http_routing::Extension(_identity): http_routing::Extension<Identity>,
) -> Result<Json<backend_status::StorageStatusResponse>, ApiError> {
    Ok(Json(state.backend.management_status().into()))
}
async fn storage_retry(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
) -> Result<Json<backend_status::StorageStatusResponse>, ApiError> {
    http_auth::storage_retry_job(state.backend, state.management_slots, identity)
        .await
        .map(backend_status::StorageStatusResponse::from)
        .map(Json)
}

async fn ready(
    State(state): State<AppState>,
) -> Result<Json<backend_status::ReadinessResponse>, ApiError> {
    let permit = state
        .management_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("management capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .ready()
            .map(backend_status::ReadinessResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
async fn capabilities(
    State(state): State<AppState>,
) -> Result<Json<backend_status::CapabilitiesResponse>, ApiError> {
    let permit = state
        .management_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("management capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state.backend.capabilities().map(|snapshot| {
            backend_status::CapabilitiesResponse::from(
                snapshot.with_upload_limit(state.config.max_upload_bytes),
            )
        })
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/server/unit/backend_admission.rs"]
mod backend_admission_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/authorized_backend.rs"]
mod authorized_backend_tests;
