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
mod backend_read;
mod backend_workspace;
pub use backend_workspace::{CreateWorkspace, PublishedUploadState, PublishedUploadStatus};
mod http_workspace;
pub use backend_read::{DirectorySelection, DirectorySnapshot, ReadDirectory};
use http_workspace::{abort, workspace, workspace_info};
mod operation_wire;
pub use backend_action::BackendAction;

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
    pub hash: String,
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
            hash: digest.finalize().to_hex().to_string(),
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
    fn management_status(&self) -> Value {
        json!({"state":"ready","retryable":false})
    }
    fn retry_storage(&self, _identity: &Identity) -> Result<Value, ApiError> {
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
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError>;
    /// Return a fully verified raw descriptor (used by SMB and local consumers).
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError>;
    /// HTTP may authenticate only requested chunks, while retaining the read lease.
    fn range_content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.content(identity, version)
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        _generation: u64,
    ) -> Result<Value, ApiError> {
        self.upload(identity, workspace, item, path, source)
    }
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
    fn observe_disks(&self, _: &InternalDiskObservation) -> Result<Value, ApiError> {
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

    fn management_status(&self) -> Value {
        self.backend.management_status()
    }
    fn retry_storage(&self, identity: &Identity) -> Result<Value, ApiError> {
        self.backend.retry_storage(&self.current(identity)?)
    }

    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        if matches!(
            action,
            crate::BackendAction::Capabilities | crate::BackendAction::Ready
        ) {
            self.backend.call(action, identity, input)
        } else {
            self.backend.call(action, &self.current(identity)?, input)
        }
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
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
    ) -> Result<Value, ApiError> {
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
    fn range_content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.backend
            .range_content(&self.current(identity)?, version)
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
    fn secure_cookie(&self) -> bool {
        self.normalized_external_origin()
            .is_ok_and(|origin| origin.is_some())
    }
    /// Match browser Origin serialization, including host case, default ports
    /// and IPv6 compression. Validation and cookie policy share this boundary.
    fn normalized_external_origin(&self) -> Result<Option<String>, ApiError> {
        self.external_origin
            .as_ref()
            .map(|origin| {
                let parsed =
                    url_encoding::address::Url::parse(origin).map_err(ApiError::invalid_source)?;
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
                Ok(parsed.origin().ascii_serialization())
            })
            .transpose()
    }
    fn validate(&self) -> Result<(), ApiError> {
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
        self.normalized_external_origin()?;
        Ok(())
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
    config: ServerConfig,
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
pub fn router(
    backend: Arc<dyn Backend>,
    auth: AuthStore,
    config: ServerConfig,
) -> Result<Router, ApiError> {
    config.validate()?;
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
    config: ServerConfig,
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
        config: config.clone(),
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
        .route("/api/backup-topologies/{topology}", get(topology))
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
    if let Some(web_dir) = config.web_dir {
        app = app.fallback_service(if web_assets_available {
            web_assets::fallback(web_dir)
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
    config.validate()?;
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
    let spool_config = config.clone();
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
async fn management_call(
    state: AppState,
    identity: Identity,
    action: crate::BackendAction,
    input: Value,
) -> Result<Json<Value>, ApiError> {
    let permit = state
        .management_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("management capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
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

macro_rules! list_handler {
    ($name:ident, $action:expr) => {
        async fn $name(
            State(state): State<AppState>,
            http_routing::Extension(identity): http_routing::Extension<Identity>,
            ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
        ) -> Result<Json<Value>, ApiError> {
            call(state, identity, $action, json!({"query": query})).await
        }
    };
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
list_handler!(repositories, crate::BackendAction::Repositories);
list_handler!(disks, crate::BackendAction::Disks);
list_handler!(events, crate::BackendAction::Events);
list_handler!(check, crate::BackendAction::Check);
read_handler!(
    repository,
    crate::BackendAction::Repository,
    "repository_id"
);
read_handler!(
    smb_binding,
    crate::BackendAction::SmbBinding,
    "repository_id"
);
read_handler!(
    default_branch,
    crate::BackendAction::DefaultBranch,
    "repository_id"
);
read_handler!(branches, crate::BackendAction::Branches, "repository_id");
read_handler!(branch_info, crate::BackendAction::Branch, "branch_id");
read_handler!(tree, crate::BackendAction::Tree, "branch_id");
read_handler!(operation, crate::BackendAction::Operation, "operation_id");
read_handler!(version, crate::BackendAction::Version, "version_id");
read_handler!(placement, crate::BackendAction::Placement, "version_id");
read_handler!(history, crate::BackendAction::History, "file_id");
read_handler!(commit_info, crate::BackendAction::Commit, "commit_id");
read_handler!(graph, crate::BackendAction::Graph, "history_tree_id");
read_handler!(topology, crate::BackendAction::Topology, "topology_id");

read_handler!(plan_info, crate::BackendAction::Plan, "plan_id");
read_handler!(policy, crate::BackendAction::Policy, "policy_id");
read_handler!(
    placement_rule,
    crate::BackendAction::PlacementRule,
    "rule_id"
);
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
post_handler!(compare, crate::BackendAction::Compare);
id_post_handler!(
    create_branch,
    crate::BackendAction::CreateBranch,
    "repository_id"
);
id_post_handler!(resolve, crate::BackendAction::Resolve, "branch_id");
id_post_handler!(
    update_smb_binding,
    crate::BackendAction::UpdateSmbBinding,
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
id_post_handler!(
    write_session,
    crate::BackendAction::WriteSession,
    "branch_id"
);
id_post_handler!(archive, crate::BackendAction::Archive, "branch_id");
id_post_handler!(namespace, crate::BackendAction::Namespace, "workspace_id");
id_post_handler!(
    commit,
    crate::BackendAction::CommitWorkspace,
    "workspace_id"
);
id_post_handler!(seal, crate::BackendAction::SealWorkspace, "workspace_id");
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
id_post_handler!(
    update_policy,
    crate::BackendAction::UpdatePolicy,
    "policy_id"
);
id_post_handler!(
    update_placement_rule,
    crate::BackendAction::UpdatePlacementRule,
    "rule_id"
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
        "storage":state.backend.management_status(),
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
) -> Result<Json<Value>, ApiError> {
    management_call(
        state,
        identity,
        crate::BackendAction::WorkspaceCapacity,
        json!({}),
    )
    .await
}

async fn storage_status(
    State(state): State<AppState>,
    http_routing::Extension(_identity): http_routing::Extension<Identity>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.backend.management_status()))
}
async fn storage_retry(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
) -> Result<Json<Value>, ApiError> {
    http_auth::storage_retry_job(state.backend, state.management_slots, identity)
        .await
        .map(Json)
}

async fn ready(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let identity = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "health".into(),
    };
    management_call(state, identity, crate::BackendAction::Ready, json!({})).await
}
async fn capabilities(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let identity = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "anonymous".into(),
    };
    let limit = state.config.max_upload_bytes;
    let Json(mut value) = management_call(
        state,
        identity,
        crate::BackendAction::Capabilities,
        json!({}),
    )
    .await?;
    let core_limit = match value.get("max_upload_bytes") {
        None => limit,
        Some(Value::String(value)) => value.parse::<u64>().map_err(ApiError::invalid_source)?,
        Some(_) => {
            return Err(ApiError::invalid(
                "backend upload capacity must be a decimal string",
            ));
        }
    };
    value["max_upload_bytes"] = limit.min(core_limit).to_string().into();
    Ok(Json(value))
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
