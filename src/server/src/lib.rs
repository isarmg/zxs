//! zxs HTTP boundary. Storage semantics stay in the synchronous core backend.
pub mod auth;
mod content_file;
pub use content_file::ContentFile;
mod backend_methods;
#[cfg(any(feature = "core", test))]
mod content_proof_budget;
#[cfg(feature = "core")]
mod disk_hardware;
#[cfg(feature = "core")]
pub mod engine_backend;
mod file_manager;
mod http_auth;
#[cfg(feature = "core")]
mod http_backup_transaction;
mod http_boundary;
mod http_disk_details;
#[cfg(feature = "core")]
mod http_integrity_scrub;
mod http_job;
use backend_methods::{declare_backend_methods, forward_authorized_backend_methods};
mod http_shutdown;
mod http_smb_account;
mod http_spool;
#[cfg(feature = "core")]
pub mod integrity_scrub;
#[cfg(feature = "core")]
pub mod management;
mod service_lifecycle;
mod shutdown;
pub mod storage_directories;
pub use shutdown::ShutdownSignal;
#[cfg(feature = "core")]
pub mod setup_recovery;
#[cfg(feature = "core")]
pub mod startup_admission;
pub use http_boundary::ApiError;
mod periodic_worker;
mod smb_account_binding;
pub mod smb_rpc;
mod spool_budget;
mod telemetry;
pub use spool_budget::SpoolLimits;
mod transfer;
mod transfer_limits;
pub use transfer_limits::TransferLimits;
mod uploads;
mod verified_chunk;
mod web_assets;

mod backend_file_history_move;
pub use backend_file_history_move::{
    MoveCurrentFile, MoveCurrentFileToEmptyDisk, MoveCurrentObject, MoveObjectTarget,
    MoveTargetName, ReplaceCurrentFile,
};
mod backend_directory_deletion;
mod backend_directory_snapshot_source;
mod http_directory_snapshot_source;
pub use backend_directory_snapshot_source::{
    DirectorySnapshotSourcePreview, ReadDirectorySnapshotSource,
};
pub use backend_directory_deletion::{
    CurrentDirectoryDeletion, DeleteCurrentDirectory, PrepareCurrentDirectoryDeletion,
    ReadDirectoryDeletions,
};
mod backend_file_deletion;
mod http_directory_deletion;
mod http_file_deletion;
pub use backend_file_deletion::{
    DeleteCurrentFile, DeleteFile, FileDeletionSummary, FileDeletionsPage, ReadFileDeletions,
    RestoreDeletedFile,
};
mod backend_file_history;
mod bounded_history_http;
mod history_window_encode;
mod history_window_http;
mod history_window_query;
mod http_content_matches;
mod http_detached_history;
mod http_directory_git;
mod http_directory_history;
mod http_directory_layout;
mod http_directory_staging;
mod http_file_history_source;
mod http_file_lifecycle;
mod http_node_retirement;
mod http_shared_history;
pub use backend_file_history::{
    ContentNodeKind, FileHistorySnapshot, HistoryBranch, HistoryNode, HistoryRevision,
    HistoryVersion, HistoryVersionData, ReadFileHistory, RenameNodeKind, RevisionKind,
};
mod backend_placement_configuration;
mod http_placement_configuration;
pub use backend_placement_configuration::{ConfigurePlacement, ReadPlacementConfiguration};
mod backend_backup_configuration;
mod http_backup_configuration;
pub use backend_backup_configuration::{ConfigureBackups, ReadBackupConfiguration};
#[cfg(feature = "core")]
pub use backend_backup_configuration::{ConfigureRecursiveBackups, RecursiveBackupConfiguration};
mod backend_archive;
mod backend_create_file_branch;
mod backend_directory_scope;
mod http_create_file_branch;
pub use backend_create_file_branch::CreateFileBranch;
mod backend_note;
mod http_note;
pub use backend_note::{
    FileNoteTarget, NodeNoteTarget, NoteNodeId, NoteTarget, UpdateNote, VersionNoteTarget,
};
mod backend_file_merge;
mod http_file_history_merge;
mod http_file_merge;
pub use backend_file_merge::{MergeFileBranches, MergeFileHistory};
mod backend_container_drafts;
mod http_container_drafts;
pub use backend_container_drafts::{
    CurrentContainerContent, ReadContainerEdit, SetContainerCommitMode,
};
mod backend_file_tags;
mod http_file_metadata;
mod http_file_tags;
pub use backend_file_tags::ReplaceContainerTags;
mod backend_disk_mkdir;
mod http_disk_mkdir;
pub use backend_disk_mkdir::CreateDiskDirectory;
mod backend_file_relocation;
mod http_file_relocation;
pub use backend_file_relocation::MoveDiskFile;
mod backend_object_move;
mod http_object_move;
pub use backend_object_move::MoveDiskObject;
mod backend_disk_write_session;
mod http_disk_write_session;
pub use backend_disk_write_session::{
    CreateDiskWriteSession, DiskWriteSession, WriteSessionBranchId,
};
mod backend_plan;
mod http_plan;
pub use backend_plan::{CancelPlan, CommitPlan};
mod backend_historical_save_plan;
mod http_historical_save_plan;
pub use backend_historical_save_plan::PrepareHistoricalSave;
mod backend_purge_plan;
mod http_purge_plan;
pub use backend_purge_plan::PreparePurge;
mod backend_split_plan;
mod http_split_plan;
pub use backend_split_plan::PrepareSplit;
mod backend_namespace_batch;
mod http_namespace_batch;
pub use backend_namespace_batch::NamespaceBatch;
mod backend_file_selection;
mod http_file_selection;
pub use backend_file_selection::{FileSelectionLocation, SelectFileBranch};
mod backend_rename_file_branch;
mod http_rename_file_branch;
pub use backend_rename_file_branch::RenameFileBranch;
mod backend_listing;
mod http_listing;
pub use backend_listing::{
    ListingContext, ListingCursor, ListingEntry, ListingLocation, ListingSnapshot, ListingVersion,
    ReadListing,
};
mod backend_disk_directory;
mod http_disk_directory;
pub use backend_directory_scope::InitializeDirectoryScope;
pub use backend_disk_directory::{DiskDirectoryAction, MutateDiskDirectory};
mod backend_create_branch;
mod backend_create_repository;
mod backend_merge_link;
mod http_merge_link;
pub use backend_merge_link::CreateMergeLink;
mod http_create_repository;
pub use backend_create_repository::CreateRepository;
mod http_create_branch;
pub use backend_archive::{ArchiveBranch, ArchiveBranchTarget};
pub use backend_create_branch::CreateBranch;
mod backend_smb_binding;
mod http_archive;
pub use backend_smb_binding::{
    ResolveSmbPath, SmbBindingStatus, SmbPathIntent, SmbPathLocation, SmbRelativePath,
    UpdateSmbBinding,
};
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
mod backend_activity;
pub use backend_activity::{
    ActivityDetailSnapshot, ActivityEventSnapshot, ActivityHistorySnapshot,
};
mod backend_events;
pub use backend_events::EventPage;
mod backend_graph;
pub use backend_graph::HistoryGraphSnapshot;
mod backend_branch;
pub use backend_branch::{BranchSnapshot, ContentDispositionSnapshot, FileBranchSnapshot};
mod backend_directory_selection;
pub use backend_directory_selection::{
    DirectorySelectionBranch, DirectorySelectionEntryInput, DirectorySelectionSnapshot,
    DirectorySelectionSourceInput, ReadDirectorySelection, SelectedDirectoryEntry,
    SelectedEntryFact,
};
mod backend_directory_scopes;
mod http_directory_selection;
pub use backend_directory_scopes::{
    DirectoryBranchChoice, DirectoryBranchFilter, DirectoryBranchProjection, DirectoryScopeBranch,
    DirectoryScopeRows, DirectoryScopesSnapshot, ReadDirectoryScopes,
};
mod backend_default_directory;
mod backend_diagnostics;
mod http_default_directory;
mod http_directory_scopes;
pub use backend_default_directory::{
    DefaultDirectoryBranchSnapshot, RenameDirectoryBranch, SelectDefaultDirectoryBranch,
    SelectedDirectoryBranch,
};
mod backend_monitoring;
pub use backend_monitoring::{
    DiskObservationFailure, DiskObservationIssue, DiskSample, PowerUnavailable,
};
mod backend_read;
mod backend_status;
pub use backend_status::{
    Capabilities, EngineReadiness, HistoryIndexStatus, Readiness, StorageState, StorageStatus,
};
mod backend_workspace;
pub use backend_workspace::{
    CreateWorkspace, PublishedUploadState, PublishedUploadStatus, StageNamespace, WorkspaceWrite,
    WorkspaceWriteSession,
};
mod http_workspace;
pub use backend_read::{
    DirectoryReadSnapshot, DirectorySelection, DirectorySnapshot, DownloadContext, ReadContent,
    ReadDirectory, ReadTree, ResolveEntry, ResolvedEntry, TreeSnapshot, VersionSnapshot,
};
use http_workspace::{abort, commit, namespace, seal, workspace, workspace_info, write_session};
mod http_read;
use http_read::resolve;
mod commit_wire;
mod http_operation;
mod operation_observation;
mod operation_wire;
use http_operation::{operation, operation_outcome};
pub use operation_observation::{CheckedOperationHealth, OperationObservation};

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
    pub fallback_files: Vec<ContentFile>,
    pub checksum_manifest: Option<storage_contract::ChecksumManifest>,
    pub file: ContentFile,
    pub size: u64,
    pub hash: String,
    pub lease: Box<dyn Send + Sync>,
    pub download_name: Option<String>,
}
impl Content {
    /// A cached byte buffer is not permission to resume an offline disk. Select
    /// an independently admitted replica before continuing an existing stream.
    pub(crate) fn check_running(&mut self) -> Result<(), ApiError> {
        let Err(source) = self.file.check_running() else {
            return Ok(());
        };
        for index in 0..self.fallback_files.len() {
            if self.fallback_files[index].check_running().is_ok() {
                std::mem::swap(&mut self.file, &mut self.fallback_files[index]);
                return Ok(());
            }
        }
        Err(ApiError::new(
            ::http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::ContentUnavailable,
            "no running content replica is available",
        )
        .with_source(source))
    }

    /// Whole-file consumers require a fully verified replica before reading.
    /// Range consumers instead verify each emitted chunk from the same buffer.
    pub fn fully_verified(mut self) -> Result<Self, ApiError> {
        self.check_running()?;
        let Some(manifest) = self.checksum_manifest.as_ref() else {
            return Ok(self);
        };
        loop {
            match verify_content_file(&mut self.file, manifest, self.size, &self.hash) {
                Ok(()) => {
                    self.checksum_manifest = None;
                    self.fallback_files.clear();
                    return Ok(self);
                }
                Err(error) => {
                    let Some(next) = self.fallback_files.pop() else {
                        return Err(ApiError::new(
                            ::http::StatusCode::SERVICE_UNAVAILABLE,
                            storage_contract::ErrorCode::ContentUnavailable,
                            "no fully verified content replica is available",
                        )
                        .with_source(error));
                    };
                    self.file = next;
                }
            }
        }
    }
}

fn verify_content_file(
    file: &mut ContentFile,
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
        for block in buffer[..length].chunks_mut(64 * 1024) {
            file.read_exact(block)?;
        }
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
    backend_methods::methods!(declare_backend_methods);

    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationOutcome, ApiError>;
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<OperationObservation, ApiError>;

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

    #[cfg(feature = "core")]
    fn backup_transaction_context(
        &self,
        _identity: &Identity,
    ) -> Result<storage_contract::backup_transaction::Context, ApiError> {
        Err(ApiError::unsupported())
    }
    #[cfg(feature = "core")]
    fn current_backup_transaction(
        &self,
        _identity: &Identity,
    ) -> Result<storage_contract::backup_transaction::Current, ApiError> {
        Err(ApiError::unsupported())
    }
    #[cfg(feature = "core")]
    fn backup_transaction_status(
        &self,
        _identity: &Identity,
        _operation: storage_contract::OperationId,
    ) -> Result<storage_contract::backup_transaction::Snapshot, ApiError> {
        Err(ApiError::unsupported())
    }
    #[cfg(feature = "core")]
    fn start_backup_transaction(
        &self,
        _identity: &Identity,
        _request: storage_contract::backup_transaction::StartRequest,
    ) -> Result<storage_contract::backup_transaction::Snapshot, ApiError> {
        Err(ApiError::unsupported())
    }
    #[cfg(feature = "core")]
    fn continue_backup_transaction(
        &self,
        _identity: &Identity,
        _operation: storage_contract::OperationId,
        _command: storage_contract::backup_transaction::Command,
    ) -> Result<storage_contract::backup_transaction::CommandResponse, ApiError> {
        Err(ApiError::unsupported())
    }

    /// Background scrub is supported only by an explicitly managed backend.
    #[cfg(feature = "core")]
    fn start_integrity_scrub(
        &self,
        _identity: &Identity,
    ) -> Result<integrity_scrub::IntegrityScrubJobId, ApiError> {
        Err(ApiError::unsupported())
    }
    #[cfg(feature = "core")]
    fn integrity_scrub_status(
        &self,
        _identity: &Identity,
    ) -> Result<integrity_scrub::IntegrityScrubStatus, ApiError> {
        Err(ApiError::unsupported())
    }
    #[cfg(feature = "core")]
    fn cancel_integrity_scrub(
        &self,
        _identity: &Identity,
        _job: integrity_scrub::IntegrityScrubJobId,
    ) -> Result<(), ApiError> {
        Err(ApiError::unsupported())
    }
    fn capabilities(&self) -> Result<Capabilities, ApiError>;
    fn ready(&self) -> Result<Readiness, ApiError>;
    fn disk_status(
        &self,
        identity: &Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, ApiError>;
    fn disk_details(
        &self,
        _identity: &Identity,
        _slot: &str,
    ) -> Result<storage_contract::disk_details::DiskDetailsRecord, ApiError> {
        Err(ApiError::unsupported())
    }
    fn update_disk_note(
        &self,
        _identity: &Identity,
        _slot: &str,
        _request: storage_contract::disk_details::DiskNoteRequest,
    ) -> Result<storage_contract::disk_details::DiskNoteResponse, ApiError> {
        Err(ApiError::unsupported())
    }
    fn move_disk_file(
        &self,
        _identity: &Identity,
        _request: MoveDiskFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        Err(ApiError::unsupported())
    }
    fn topology_snapshot(&self, identity: &Identity) -> Result<TopologySnapshot, ApiError>;
    fn event_page(&self, identity: &Identity, after: u64) -> Result<EventPage, ApiError>;
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
    backend_methods::methods!(forward_authorized_backend_methods);

    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<OperationObservation, ApiError> {
        self.backend
            .operation(&self.current(identity)?, operation_id)
    }
    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationOutcome, ApiError> {
        self.backend
            .operation_outcome(&self.current(identity)?, operation_id)
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        self.backend.capabilities()
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        self.backend.ready()
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        self.backend.disk_status(&self.current(identity)?)
    }
    fn disk_details(
        &self,
        identity: &Identity,
        slot: &str,
    ) -> Result<storage_contract::disk_details::DiskDetailsRecord, ApiError> {
        self.backend.disk_details(&self.current(identity)?, slot)
    }
    fn update_disk_note(
        &self,
        identity: &Identity,
        slot: &str,
        request: storage_contract::disk_details::DiskNoteRequest,
    ) -> Result<storage_contract::disk_details::DiskNoteResponse, ApiError> {
        self.backend
            .update_disk_note(&self.current(identity)?, slot, request)
    }
    fn move_disk_file(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MoveDiskFile,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.backend
            .move_disk_file(&self.current(identity)?, request)
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        self.backend.topology_snapshot(&self.current(identity)?)
    }
    fn event_page(
        &self,
        identity: &crate::auth::Identity,
        after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        self.backend.event_page(&self.current(identity)?, after)
    }

    #[cfg(feature = "core")]
    fn start_integrity_scrub(
        &self,
        identity: &Identity,
    ) -> Result<integrity_scrub::IntegrityScrubJobId, ApiError> {
        self.backend.start_integrity_scrub(&self.current(identity)?)
    }
    #[cfg(feature = "core")]
    fn integrity_scrub_status(
        &self,
        identity: &Identity,
    ) -> Result<integrity_scrub::IntegrityScrubStatus, ApiError> {
        self.backend
            .integrity_scrub_status(&self.current(identity)?)
    }
    #[cfg(feature = "core")]
    fn cancel_integrity_scrub(
        &self,
        identity: &Identity,
        job: integrity_scrub::IntegrityScrubJobId,
    ) -> Result<(), ApiError> {
        self.backend
            .cancel_integrity_scrub(&self.current(identity)?, job)
    }

    #[cfg(feature = "core")]
    fn backup_transaction_context(
        &self,
        identity: &Identity,
    ) -> Result<storage_contract::backup_transaction::Context, ApiError> {
        self.backend
            .backup_transaction_context(&self.current(identity)?)
    }
    #[cfg(feature = "core")]
    fn current_backup_transaction(
        &self,
        identity: &Identity,
    ) -> Result<storage_contract::backup_transaction::Current, ApiError> {
        self.backend
            .current_backup_transaction(&self.current(identity)?)
    }
    #[cfg(feature = "core")]
    fn backup_transaction_status(
        &self,
        identity: &Identity,
        operation: storage_contract::OperationId,
    ) -> Result<storage_contract::backup_transaction::Snapshot, ApiError> {
        self.backend
            .backup_transaction_status(&self.current(identity)?, operation)
    }
    #[cfg(feature = "core")]
    fn start_backup_transaction(
        &self,
        identity: &Identity,
        request: storage_contract::backup_transaction::StartRequest,
    ) -> Result<storage_contract::backup_transaction::Snapshot, ApiError> {
        self.backend
            .start_backup_transaction(&self.current(identity)?, request)
    }
    #[cfg(feature = "core")]
    fn continue_backup_transaction(
        &self,
        identity: &Identity,
        operation: storage_contract::OperationId,
        command: storage_contract::backup_transaction::Command,
    ) -> Result<storage_contract::backup_transaction::CommandResponse, ApiError> {
        self.backend
            .continue_backup_transaction(&self.current(identity)?, operation, command)
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
}

#[derive(Clone)]
pub struct SmbConfig {
    pub socket: PathBuf,
    /// Durable system-disk storage, independent of the disposable socket directory.
    pub spool_dir: PathBuf,
    pub allowed_uid: u32,
    pub spool_limits: SpoolLimits,
    /// Initial non-guest Samba account; persisted selection overrides it on restart.
    pub username: String,
    /// Additional existing Samba accounts provisioned for every ZXS share.
    pub additional_users: Vec<String>,
}
#[derive(Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    /// Required when TLS terminates upstream. Origin comparison never uses forwarded headers.
    pub external_origin: Option<String>,
    /// Exact proxy peers allowed to supply a single, overwritten X-Forwarded-For IP.
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
    /// Default HTTP configuration listening on all IPv4 interfaces, port 7400.
    pub fn new(root: &Path) -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 7400)),
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
        if !self.trusted_proxy_ips.is_empty() && self.external_origin.is_none() {
            return Err(ApiError::invalid(
                "trusted proxies require an HTTPS external origin",
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
        if let Some(smb) = &self.smb {
            smb_account_binding::candidates(smb)?;
        }
        let external_origin = self
            .external_origin
            .as_deref()
            .map(|origin| {
                authentication::ExternalOrigin::parse(origin).map_err(ApiError::invalid_source)
            })
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
    external_origin: Option<authentication::ExternalOrigin>,
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

// A conservative, per-router bound for general HTTP backend jobs. This is
// independent of content/upload admission and is not a process-wide I/O budget.
const HTTP_BACKEND_SLOTS: usize = 8;

#[derive(Clone)]
struct AppState {
    auth_reader: auth::AuthReader,
    backend: Arc<dyn Backend>,
    auth: Arc<Mutex<AuthStore>>,
    config: Arc<AdmittedServerConfig>,
    login_admission: Arc<http_auth::LoginAdmission>,
    password_attempts: Arc<Mutex<HashMap<String, http_auth::Attempts>>>,
    #[cfg(feature = "core")]
    setup_attempts: Arc<Mutex<HashMap<String, http_auth::Attempts>>>,
    #[cfg(feature = "core")]
    recovery_attempts: Arc<Mutex<HashMap<String, http_auth::Attempts>>>,
    password_slots: Arc<tokio::sync::Semaphore>,
    smb_password_slots: Arc<tokio::sync::Semaphore>,
    smb_account: Option<Arc<smb_account_binding::Authority>>,
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
    let shutdown = Arc::new(service_lifecycle::EmbeddedManagementShutdown::new(
        Arc::clone(&runtime.backend),
        Arc::clone(&runtime.telemetry),
    ));
    router_shared(
        runtime.backend,
        runtime.auth,
        config,
        runtime.telemetry,
        spool_budget,
        runtime.smb_account,
    )
    .map(|router| router.layer(http_routing::Extension(shutdown)))
}

/// Common initialization for embedded routers and the network server. Callers
/// validate first; serve binds HTTP before starting background management work.
struct ManagementRuntime {
    smb_account: Option<Arc<smb_account_binding::Authority>>,
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
        let smb_account =
            config
                .smb
                .as_ref()
                .and_then(|smb| match smb_account_binding::Authority::open(smb) {
                    Ok(authority) => Some(authority),
                    Err(error) => {
                        tracing::error!(
                            error = ?error,
                            "SMB account binding unavailable; management HTTP remains available"
                        );
                        None
                    }
                });
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
            smb_account,
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
    smb_account: Option<Arc<smb_account_binding::Authority>>,
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
    let static_etags = if config.web_dir.is_none() {
        web_assets::embedded::etags()
    } else {
        static_etags
    };
    let web_assets_available = static_etags.is_some();
    let auth_reader = lock(&auth).reader();
    let state = AppState {
        smb_account,
        auth_reader,
        backend,
        auth,
        telemetry,
        config: Arc::clone(&config),
        spool_budget,
        login_admission: Arc::new(http_auth::LoginAdmission::default()),
        password_attempts: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(feature = "core")]
        setup_attempts: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(feature = "core")]
        recovery_attempts: Arc::new(Mutex::new(HashMap::new())),
        password_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        smb_password_slots: Arc::new(tokio::sync::Semaphore::new(1)),
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
    let protected = protected
        .merge(setup_recovery::protected_routes())
        .merge(http_integrity_scrub::routes())
        .merge(http_backup_transaction::routes());
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
            "/api/smb/account",
            get(http_smb_account::account).post(http_smb_account::select_account),
        )
        .route("/api/smb/password", post(http_smb_account::change_password))
        .route(
            "/api/repositories",
            get(repositories).post(http_create_repository::create),
        )
        .route("/api/repositories/{repo}", get(repository))
        .route(
            "/api/repositories/{repo}/smb-binding",
            get(smb_binding).put(update_smb_binding),
        )
        .route(
            "/api/repositories/{repo}/default-branch",
            get(default_branch).put(http_default_directory::select),
        )
        .route(
            "/api/repositories/{repo}/directories/{directory}/history",
            get(http_directory_history::nodes),
        )
        .route(
            "/api/repositories/{repo}/directories/{directory}/history/branches",
            get(http_directory_history::branches),
        )
        .route(
            "/api/repositories/{repo}/directory-git",
            get(http_directory_git::read),
        )
        .route(
            "/api/repositories/{repo}/directory-layout/reconcile",
            post(http_directory_layout::reconcile),
        )
        .route(
            "/api/repositories/{repo}/directory-git/initialize",
            post(http_directory_git::initialize),
        )
        .route(
            "/api/repositories/{repo}/directory-git/children/stage",
            post(http_directory_git::stage_child),
        )
        .route(
            "/api/repositories/{repo}/directory-git/stage",
            post(http_directory_git::stage_owned),
        )
        .route(
            "/api/repositories/{repo}/directory-branches",
            get(http_directory_scopes::read).post(http_directory_selection::read),
        )
        .route(
            "/api/repositories/{repo}/directory-branches/{id}/staging",
            get(http_directory_staging::read),
        )
        .route(
            "/api/repositories/{repo}/directory-branches/{id}/commits",
            post(http_directory_staging::commit),
        )
        .route(
            "/api/repositories/{repo}/branches",
            get(branches).post(http_create_branch::create),
        )
        .route("/api/branches/{branch}/tree", get(tree))
        .route("/api/branches/{branch}", get(branch_info))
        .route("/api/branches/{branch}/resolve", post(resolve))
        .route("/api/branches/{branch}/write-sessions", post(write_session))
        .route(
            "/api/branches/{branch}/archive",
            post(http_archive::archive),
        )
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
            "/api/operations/{operation}/outcome",
            get(operation_outcome),
        )
        .route(
            "/api/versions/{version}/content",
            get(content).head(content),
        )
        .route("/api/versions/{version}", get(version))
        .route("/api/versions/{version}/placement", get(placement))
        .route("/api/versions/{version}/checksums", get(checksums))
        .route(
            "/api/file-deletion-batches/{operation}",
            get(http_file_deletion::batch_preview),
        )
        .route(
            "/api/file-deletion-batches/undo",
            post(http_file_deletion::undo_batch),
        )
        .route(
            "/api/files/{file}/deletion/summary",
            get(http_file_deletion::review::summary),
        )
        .route(
            "/api/files/{file}/deletion/branches",
            get(http_file_deletion::review::branches),
        )
        .route(
            "/api/files/{file}/deletion/parents",
            get(http_file_deletion::review::parents),
        )
        .route(
            "/api/files/{file}/deletion/undo-summary",
            post(http_file_deletion::review::undo),
        )
        .route(
            "/api/file-deletion-undos/{operation}",
            get(http_file_deletion::review::outcome),
        )
        .route("/api/file-deletions", get(http_file_deletion::list))
        .route(
            "/api/files/{file}/deletion",
            get(http_file_deletion::read).post(http_file_deletion::submit),
        )
        .route(
            "/api/files/{file}/deletion/undo",
            post(http_file_deletion::undo),
        )
        .route(
            "/api/repositories/{repository}/directory-deletions/{owner}",
            get(http_directory_deletion::list),
        )
        .route(
            "/api/repositories/{repository}/directory-deletions/{owner}/{directory}",
            get(http_directory_deletion::read),
        )
        .route(
            "/api/repositories/{repository}/directory-deletions/undo",
            post(http_directory_deletion::undo),
        )
        .route(
            "/api/files/{file}/history",
            get(history_window_http::versions),
        )
        .route(
            "/api/files/{file}/history/versions/{version}/sources",
            get(history_window_http::version_sources),
        )
        .route("/api/commits/{commit}", get(commit_info))
        .route("/api/history-trees/{history}/graph", get(graph))
        .route(
            "/api/history-trees/{history}/merge-links",
            post(http_merge_link::create),
        )
        .route(
            "/api/history-trees/{history}/purge-plans",
            post(http_purge_plan::prepare),
        )
        .route(
            "/api/history-trees/{history}/split-plans",
            post(http_split_plan::prepare),
        )
        .route(
            "/api/history-trees/{history}/historical-save-plans",
            post(http_historical_save_plan::prepare),
        )
        .route("/api/plans/{plan}/commit", post(http_plan::commit))
        .route("/api/plans/{plan}", get(http_plan::info))
        .route("/api/plans/{plan}/cancel", post(http_plan::cancel))
        .route("/api/admin/disks", get(disks).post(register_disk))
        .route(
            "/api/admin/disks/{slot}/details",
            get(http_disk_details::read),
        )
        .route(
            "/api/admin/disks/{slot}/note",
            put(http_disk_details::update_note),
        )
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
    if config.web_dir.is_none() && web_assets_available {
        app = app.fallback_service(web_assets::embedded::fallback());
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
        smb_account,
        backend,
        auth,
        telemetry,
    } = ManagementRuntime::start(backend, auth, &config)?;
    management_shutdown.set_telemetry(Arc::clone(&telemetry));
    // Keep optional bridge setup isolated from management lifecycle failures.
    let smb = config.smb.clone();
    let smb_max_file_bytes = config.max_upload_bytes;
    let bridge_auth = auth.clone();
    let spool_config = Arc::clone(&config);
    let bridge_account = smb_account.clone();
    let app = router_shared(
        backend.clone(),
        auth,
        config,
        telemetry,
        spool_budget,
        smb_account,
    )?;
    let http = |stopped| http_shutdown::serve(listener, app, stopped);
    let smb_connections = management_shutdown.smb_connections();
    let bridge = async move {
        if let (Some(smb), Some(bridge_account)) = (smb, bridge_account) {
            let run = async move {
                smb_rpc::serve_owned(
                    smb,
                    bridge_auth,
                    backend,
                    smb_max_file_bytes,
                    smb_connections,
                    bridge_account,
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

pub(crate) const JSON_BODY_LIMIT: usize = 1024 * 1024;

async fn json_body(request: Request) -> Result<Value, ApiError> {
    let bytes = http_boundary::read_body(request, JSON_BODY_LIMIT).await?;
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

async fn repositories(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_read::RepositoryListResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .repositories_snapshot(&identity)
                .map(|repositories| backend_read::RepositoryListResponse {
                    repositories: repositories
                        .into_iter()
                        .map(backend_read::RepositoryResponse::from)
                        .collect(),
                })
        })
        .await
        .map(Json)
}
async fn disks(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_diagnostics::DisksResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .disk_status(&identity)
                .map(backend_diagnostics::DisksResponse::from)
        })
        .await
        .map(Json)
}
async fn events(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_events::EventPageResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let after = storage_contract::parse_u64_decimal(
                query.get("after").map(String::as_str).unwrap_or("0"),
            )
            .map_err(ApiError::invalid_source)?;
            backend
                .event_page(&identity, after)
                .map(backend_events::EventPageResponse::from)
        })
        .await
        .map(Json)
}
async fn check(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_diagnostics::CheckResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .integrity_check(&identity)
                .map(backend_diagnostics::CheckResponse::from)
        })
        .await
        .map(Json)
}
async fn repository(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_read::RepositoryResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .repository_snapshot(&identity, &id)
                .map(backend_read::RepositoryResponse::from)
        })
        .await
        .map(Json)
}

async fn default_branch(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_default_directory::DefaultDirectoryBranchResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .default_directory_branch(&identity, &id)
                .map(backend_default_directory::DefaultDirectoryBranchResponse::from)
        })
        .await
        .map(Json)
}

async fn branch_info(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_branch::BranchResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .branch_snapshot(&identity, &id)
                .map(backend_branch::BranchResponse::from)
        })
        .await
        .map(Json)
}
async fn branches(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_branch::BranchListResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend.branches_snapshot(&identity, &id).map(|branches| {
                backend_branch::BranchListResponse {
                    branches: branches
                        .into_iter()
                        .map(backend_branch::BranchResponse::from)
                        .collect(),
                }
            })
        })
        .await
        .map(Json)
}
async fn tree(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(branch): ApiPath<String>,
    ApiQuery(mut query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<TreeSnapshot>, ApiError> {
    let job =
        crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?;
    let commit = query.remove("commit_id");
    job.run(move |backend| {
        backend.tree_snapshot(
            &identity,
            ReadTree {
                branch: &branch,
                commit: commit.as_deref(),
            },
        )
    })
    .await
    .map(Json)
}
async fn version(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_read::VersionResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .version_snapshot(&identity, &id)
                .map(backend_read::VersionResponse::from)
        })
        .await
        .map(Json)
}
async fn placement(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(version): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_storage_metadata::PlacementResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .placement_snapshot(&identity, &version)
                .map(backend_storage_metadata::PlacementResponse::from)
        })
        .await
        .map(Json)
}
async fn commit_info(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(commit): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<commit_wire::CommitResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .commit_snapshot(&identity, &commit)
                .map(commit_wire::CommitResponse::from)
        })
        .await
        .map(Json)
}
async fn graph(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(history): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_graph::HistoryGraphResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .history_graph(&identity, &history)
                .map(backend_graph::HistoryGraphResponse::from)
        })
        .await
        .map(Json)
}
async fn topology(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_storage_metadata::TopologyResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .topology_snapshot(&identity)
                .map(backend_storage_metadata::TopologyResponse::from)
        })
        .await
        .map(Json)
}

async fn register_disk(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let input = json_body(request).await?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| {
            ApiError::limit("management operation capacity reached").with_source(source)
        })?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend.setup_operation("disk_add_commit", input, None, Some(&identity))
        })
        .await
        .map(Json)
}

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
            "web_assets": if state.web_assets_available { "ready" }
                else if state.config.web_dir.is_none() { "not_configured" } else { "unavailable" }
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
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| backend.workspace_capacity(&identity))
        .await
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
    http_auth::storage_retry_job(state.backend, state.auth, state.management_slots, identity)
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

#[cfg(all(test, feature = "core"))]
#[path = "../../../test/server/unit/projection_failure.rs"]
mod projection_failure;

#[cfg(test)]
#[path = "../../../test/server/unit/backend_admission.rs"]
mod backend_admission_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/authorized_backend.rs"]
mod authorized_backend_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/container_tags.rs"]
mod container_tags_tests;
