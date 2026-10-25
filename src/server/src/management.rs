//! System-disk control plane. Engine recovery is optional and never gates HTTP.
//! `management-auth.json` is primary; engine snapshots are disaster-recovery
//! copies. Only explicit authorized recovery can install them; background
//! recovery and restarts never replace missing or damaged system credentials.
use crate::{
    ApiError, Backend, BackendAction, Content, InternalDiskObservation,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
use json_codec::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageState {
    Ready,
    ReadOnly,
    Unavailable,
    Unconfigured,
    RecoveryRequired,
}
impl StorageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::ReadOnly => "read_only",
            Self::Unavailable => "unavailable",
            Self::Unconfigured => "unconfigured",
            Self::RecoveryRequired => "recovery_required",
        }
    }
}
struct State {
    backend: Option<Arc<EngineBackend>>,
    status: StorageState,
    admitted: bool,
    observed_at: std::time::Instant,
}
impl State {
    fn effective_status(&self) -> StorageState {
        if matches!(self.status, StorageState::Ready | StorageState::ReadOnly)
            && self.observed_at.elapsed() >= std::time::Duration::from_secs(15)
        {
            StorageState::Unavailable
        } else {
            self.status
        }
    }
}
struct Inner {
    data_root: PathBuf,
    management_root: PathBuf,
    state: Mutex<State>,
    auth: Mutex<Option<Arc<Mutex<AuthStore>>>>,
    recovering: AtomicBool,
    stopping: AtomicBool,
    recovery_worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    generation: AtomicU64,
    disk_observation: Mutex<Option<(std::time::Instant, Value)>>,
    capabilities: Mutex<Value>,
    setup_disks: BTreeMap<String, PathBuf>,
    setup_configs: BTreeMap<String, PathBuf>,
    allow_development_setup: bool,
    retained_workspace_limits: Option<storage_engine::RetainedWorkspaceLimits>,
}
pub struct ManagementBackend {
    inner: Arc<Inner>,
}
/// Locally provisioned setup choices and immutable engine policy. Paths are
/// retained without reading them; unavailable presets never gate management startup.
#[derive(Default)]
pub struct ManagementOptions {
    pub setup_disks: BTreeMap<String, PathBuf>,
    pub setup_configs: BTreeMap<String, PathBuf>,
    pub allow_development_setup: bool,
    /// Immutable host policy, never loaded from an HTTP request or persisted to the pool.
    pub retained_workspace_limits: Option<storage_engine::RetainedWorkspaceLimits>,
}

/// Marks the old engine snapshot as a replica, never an automatic admission source.
pub fn claim_authority(data_root: &Path) -> Result<(), ApiError> {
    let path = data_root.join("management-authority.json");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(b"{\"authority\":\"system-management-auth\"}\n")
                .map_err(ApiError::io)?;
            file.sync_all().map_err(ApiError::io)?;
            fs::File::open(data_root)
                .and_then(|dir| dir.sync_all())
                .map_err(ApiError::io)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let marker = fs::File::from(
                rustix::fs::open(
                    &path,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::NONBLOCK
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|error| ApiError::io(error.into()))?,
            );
            let metadata = marker.metadata().map_err(ApiError::io)?;
            if !metadata.is_file() {
                return Err(ApiError::invalid("invalid management authority marker"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
                    return Err(ApiError::invalid("unsafe management authority marker"));
                }
            }
            // A prior interrupted claim may have left an unsynced file. The
            // fence is valid only after both file and directory are durable.
            marker.sync_all().map_err(ApiError::io)?;
            fs::File::open(data_root)
                .and_then(|directory| directory.sync_all())
                .map_err(ApiError::io)?;
            Ok(())
        }
        Err(error) => Err(ApiError::io(error)),
    }
}

/// Local credentials load before background disk discovery. A corrupt/missing
/// store becomes a closed recovery state, never a fresh-install administrator.
pub fn open_service(
    data_root: &Path,
    management_root: &Path,
) -> Result<(Arc<ManagementBackend>, AuthStore), ApiError> {
    open_service_with_disks(data_root, management_root, BTreeMap::new(), false)
}
pub fn open_service_with_disks(
    data_root: &Path,
    management_root: &Path,
    setup_disks: BTreeMap<String, PathBuf>,
    allow_development_setup: bool,
) -> Result<(Arc<ManagementBackend>, AuthStore), ApiError> {
    open_service_with_options(
        data_root,
        management_root,
        ManagementOptions {
            setup_disks,
            setup_configs: BTreeMap::new(),
            allow_development_setup,
            retained_workspace_limits: None,
        },
    )
}
pub fn open_service_with_options(
    data_root: &Path,
    management_root: &Path,
    options: ManagementOptions,
) -> Result<(Arc<ManagementBackend>, AuthStore), ApiError> {
    let ManagementOptions {
        setup_disks,
        setup_configs,
        allow_development_setup,
        retained_workspace_limits,
    } = options;
    // Pure host validation must precede every system-state write. Baseline
    // comparison belongs to the background engine open, not startup disk I/O.
    if let Some(limits) = retained_workspace_limits {
        limits.validate().map_err(ApiError::invalid)?;
    }
    // These are system state directories, never data-disk mountpoints.
    if !setup_disks.is_empty() || !setup_configs.is_empty() {
        create_private_directory(data_root)?;
    }
    create_private_directory(management_root)?;
    let mut auth = AuthStore::open_management(management_root).unwrap_or_else(|error| {
        tracing::warn!(code=%error.code, "management credentials require recovery");
        AuthStore::unavailable(management_root)
    });
    if auth.is_available() {
        // Record local authority before accepting any credential mutation,
        // including when the data pool itself cannot be opened.
        let claim = create_private_directory(data_root).and_then(|_| claim_authority(data_root));
        if let Err(error) = claim {
            tracing::warn!(code=%error.code,"cannot durably establish management authority");
            auth = AuthStore::unavailable(management_root);
        }
    }
    let backend = Arc::new(ManagementBackend {
        inner: Arc::new(Inner {
            data_root: data_root.into(),
            management_root: management_root.into(),
            state: Mutex::new(State {
                backend: None,
                status: StorageState::Unavailable,
                admitted: false,
                observed_at: std::time::Instant::now(),
            }),
            auth: Mutex::new(None),
            recovering: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            recovery_worker: Mutex::new(None),
            generation: AtomicU64::new(0),
            disk_observation: Mutex::new(None),
            capabilities: Mutex::new(json!({"features":[],"max_upload_bytes":"0"})),
            setup_disks,
            setup_configs,
            allow_development_setup,
            retained_workspace_limits,
        }),
    });
    Ok((backend, auth))
}
fn create_private_directory(path: &Path) -> Result<(), ApiError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(ApiError::io)
}
struct RecoveryPermit<'a>(&'a AtomicBool);
impl Drop for RecoveryPermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
impl Inner {
    fn set(&self, backend: Option<Arc<EngineBackend>>, status: StorageState) {
        if let Ok(mut state) = self.state.lock() {
            state.backend = backend;
            state.status = if self.stopping.load(Ordering::SeqCst) {
                StorageState::Unavailable
            } else {
                status
            };
            state.observed_at = std::time::Instant::now();
            state.admitted = !self.stopping.load(Ordering::SeqCst)
                && matches!(status, StorageState::Ready | StorageState::ReadOnly);
        }
    }
    fn recover(self: &Arc<Self>) -> Result<(), ApiError> {
        self.start_recovery(false)
    }
    fn resume_after_setup(self: &Arc<Self>, mut completed: Value) -> Result<Value, ApiError> {
        match self.recover() {
            Ok(()) => Ok(completed),
            Err(error) if error.code == storage_contract::ErrorCode::StorageRetryRunning => {
                Ok(completed)
            }
            Err(error) if error.code == storage_contract::ErrorCode::ServiceStopping => {
                // The admitted setup operation is already durable. Shutdown
                // must not turn its success into a misleading retryable failure.
                completed["recovering"] = json!(false);
                completed["restart_required"] = json!(true);
                Ok(completed)
            }
            Err(error) => {
                // Initialization/recovery has committed already. An enqueue
                // failure is a service-restart requirement, not an operation
                // failure that invites repeating the durable mutation.
                tracing::warn!(%error, "completed setup requires a service restart");
                completed["recovering"] = json!(false);
                completed["restart_required"] = json!(true);
                Ok(completed)
            }
        }
    }
    fn start_recovery(self: &Arc<Self>, reopen: bool) -> Result<(), ApiError> {
        // Serialize enqueue with stop, including ownership of a newly spawned
        // native thread. No thread may escape the shutdown owner's drain.
        let mut worker = crate::lock(&self.recovery_worker)?;
        self.ensure_running()?;
        if self.recovering.swap(true, Ordering::SeqCst) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::StorageRetryRunning,
                "storage recovery is already running",
            ));
        }
        let previous = worker.take();
        drop(worker);
        // A previous worker has relinquished recovering under the same mutex.
        // Join outside all locks, even if it is still unwinding its epilogue.
        if let Some(previous) = previous {
            let _ = previous.join();
        }
        if reopen {
            let mut state = match crate::lock(&self.state) {
                Ok(state) => state,
                Err(error) => {
                    self.recovering.store(false, Ordering::SeqCst);
                    return Err(error);
                }
            };
            state.status = state.effective_status();
            if state.status != StorageState::Ready {
                state.admitted = false;
                if state
                    .backend
                    .as_ref()
                    .is_some_and(|backend| Arc::strong_count(backend) > 1)
                {
                    self.recovering.store(false, Ordering::SeqCst);
                    let mut error = ApiError::conflict(
                        storage_contract::ErrorCode::StorageDraining,
                        "existing storage operations must finish before reopening; retry after they drain",
                    );
                    error.retryable = true;
                    return Err(error);
                }
                state.backend = None;
            }
        }
        let mut worker = crate::lock(&self.recovery_worker)?;
        if let Err(error) = self.ensure_running() {
            self.recovering.store(false, Ordering::SeqCst);
            return Err(error);
        }
        let inner = self.clone();
        match std::thread::Builder::new()
            .name("zxs-storage-recovery".into())
            .spawn(move || {
                inner.run_recovery(Self::recover_once);
            }) {
            Ok(handle) => *worker = Some(handle),
            Err(error) => {
                self.recovering.store(false, Ordering::SeqCst);
                return Err(ApiError::io(error));
            }
        }
        Ok(())
    }
    fn ensure_running(&self) -> Result<(), ApiError> {
        if self.stopping.load(Ordering::SeqCst) {
            Err(ApiError::new(
                503,
                storage_contract::ErrorCode::ServiceStopping,
                "service is stopping",
            ))
        } else {
            Ok(())
        }
    }
    fn stop(&self) {
        // Recover poison during shutdown: a panic must not detach a writer.
        let worker = self
            .recovery_worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.stopping.store(true, Ordering::SeqCst);
        drop(worker);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.admitted = false;
        state.status = StorageState::Unavailable;
    }
    fn drain(&self) -> Result<(), ApiError> {
        self.stop();
        let worker = self
            .recovery_worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(worker) = worker {
            worker.join().map_err(|_| {
                ApiError::new(
                    500,
                    storage_contract::ErrorCode::RecoveryWorkerFailed,
                    "storage recovery worker panicked",
                )
            })?;
        }
        Ok(())
    }
    fn run_recovery(self: Arc<Self>, mut recover: impl FnMut(&Arc<Self>) -> Result<(), ApiError>) {
        loop {
            let generation = self.generation.load(Ordering::SeqCst);
            if let Err(error) = recover(&self) {
                tracing::warn!(code=%error.code,"storage unavailable; management remains online");
                if let Ok(mut state) = self.state.lock() {
                    state.admitted = false;
                    state.status = StorageState::RecoveryRequired;
                }
            }
            // Coalesce auth changes in this owned thread instead of recursively
            // spawning a replacement that the shutdown owner might miss.
            let _worker = self
                .recovery_worker
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !self.stopping.load(Ordering::SeqCst)
                && self.generation.load(Ordering::SeqCst) != generation
            {
                continue;
            }
            self.recovering.store(false, Ordering::SeqCst);
            return;
        }
    }
    fn recover_once(self: &Arc<Self>) -> Result<(), ApiError> {
        let existing = crate::lock(&self.state)?.backend.clone();
        let backend = if let Some(backend) = existing {
            backend
        } else {
            let engine = match storage_engine::Engine::open_with_workspace_limits(
                &self.data_root,
                self.retained_workspace_limits,
            ) {
                Ok(engine) => engine,
                Err(error) => {
                    tracing::warn!(%error,"storage recovery failed");
                    let status =
                        if self.data_root.join("config.json").try_exists().ok() == Some(false) {
                            if self.data_root.join("management-authority.json").exists() {
                                StorageState::RecoveryRequired
                            } else {
                                StorageState::Unconfigured
                            }
                        } else {
                            StorageState::Unavailable
                        };
                    self.set(None, status);
                    return Ok(());
                }
            };
            Arc::new(EngineBackend::new(engine))
        };
        let auth = crate::lock(&self.auth)?.clone().ok_or_else(|| {
            ApiError::new(
                503,
                storage_contract::ErrorCode::AuthRecoveryRequired,
                "management is not initialized",
            )
        })?;
        {
            let mut auth = crate::lock(&auth)?;
            // A recovered pool is evidence, never permission to install system
            // credentials. In particular, disk restore may have completed before
            // bootstrap authorization expired or was revoked. Neither an absent
            // handover marker nor a restart may turn that rejection into consent.
            if !auth.is_available() {
                return Err(ApiError::new(
                    503,
                    storage_contract::ErrorCode::AuthRecoveryRequired,
                    "system credentials require explicit local recovery",
                ));
            }
            claim_authority(&self.data_root)?;
            let mut initial = auth.snapshot()?;
            initial["sessions"] = json!({});
            let last = Mutex::new(initial);
            let weak = Arc::downgrade(self);
            auth.attach_replica(Arc::new(move |mut snapshot| {
                snapshot["sessions"] = json!({});
                let mut last = crate::lock(&last)?;
                if *last == snapshot {
                    return Ok(());
                }
                *last = snapshot;
                if let Some(inner) = weak.upgrade() {
                    // Revoke data admission synchronously. Only the bounded,
                    // coalescing worker touches the engine/data disks.
                    {
                        let mut state = crate::lock(&inner.state)?;
                        state.admitted = false;
                        state.status = StorageState::RecoveryRequired;
                    }
                    inner.generation.fetch_add(1, Ordering::SeqCst);
                    let _ = inner.recover();
                }
                Ok(())
            }));
        }
        backend.set_management_auth(Arc::downgrade(&auth))?;
        self.set(Some(backend.clone()), StorageState::RecoveryRequired);
        loop {
            let mut snapshot = crate::lock(&auth)?.snapshot()?;
            snapshot["sessions"] = json!({});
            backend.replicate_auth(snapshot.clone())?;
            let capabilities = backend.call(
                BackendAction::Capabilities,
                &Identity {
                    admission_session: None,
                    smb_binding: None,
                    principal: "management".into(),
                },
                json!({}),
            )?;
            *crate::lock(&self.capabilities)? = capabilities;
            let ready = if backend.read_only()? {
                StorageState::ReadOnly
            } else {
                StorageState::Ready
            };
            let current = crate::lock(&auth)?;
            let mut latest = current.snapshot()?;
            latest["sessions"] = json!({});
            if latest == snapshot {
                // Same auth->state order as the callback; no engine lock held.
                self.set(Some(backend.clone()), ready);
                return Ok(());
            }
        }
    }
    fn backend(&self) -> Result<Arc<EngineBackend>, ApiError> {
        self.ensure_running()?;
        let state = crate::lock(&self.state)?;
        state.backend.as_ref().filter(|_|state.admitted && state.status!=StorageState::Unavailable && state.observed_at.elapsed()<std::time::Duration::from_secs(15)).cloned().ok_or_else(||ApiError::new(503,storage_contract::ErrorCode::StorageUnavailable,"storage is unavailable; inspect /api/system/storage and retry after correcting the fault"))
    }
    /// Keep the engine lease inside one sample. The observer's idle interval
    /// must not look like an in-flight storage operation to recovery admission.
    fn observe_storage_once(&self) {
        let backend = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.backend.as_ref().filter(|_| state.admitted).cloned());
        let observation = backend
            .as_ref()
            .ok_or_else(|| {
                ApiError::new(
                    503,
                    storage_contract::ErrorCode::StorageUnavailable,
                    "storage unavailable",
                )
            })
            .and_then(|backend| {
                let health = backend.call(
                    BackendAction::Ready,
                    &Identity {
                        admission_session: None,
                        smb_binding: None,
                        principal: "monitor".into(),
                    },
                    json!({}),
                );
                if let Ok(mut state) = self.state.lock()
                    && state.admitted
                    && state
                        .backend
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, backend))
                {
                    state.observed_at = std::time::Instant::now();
                    state.status = match &health {
                        Ok(_) => StorageState::Ready,
                        Err(error)
                            if matches!(
                                error.code,
                                storage_contract::ErrorCode::RecoveryRequired
                                    | storage_contract::ErrorCode::MetadataUnavailable
                            ) =>
                        {
                            StorageState::ReadOnly
                        }
                        Err(_) => StorageState::Unavailable,
                    };
                }
                backend.observe_disks(&InternalDiskObservation(()))
            });
        if let Ok(mut cache) = self.disk_observation.lock() {
            *cache = observation
                .ok()
                .map(|value| (std::time::Instant::now(), value));
        }
    }
}
impl Backend for ManagementBackend {
    fn setup_operation(
        &self,
        action: &'static str,
        input: Value,
        bootstrap: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<Value, ApiError> {
        self.inner.ensure_running()?;
        use crate::setup_recovery::{
            CompleteSetupRequest, PresetPreviewRequest, PresetSetupRequest, RecoveryCommitRequest,
            RecoveryPreviewRequest, RestoreRequest, SetupRecovery, SetupRequest,
        };
        // Authenticate against system-disk state before touching any data path.
        let auth = crate::lock(&self.inner.auth)?
            .clone()
            .ok_or_else(ApiError::unauthorized)?;
        let identity = identity
            .map(|identity| {
                // Recovery is an HTTP management operation, never an SMB
                // principal-only operation. Require its current login session.
                if identity.admission_session.is_none() {
                    return Err(ApiError::unauthorized());
                }
                crate::lock(&auth)?.revalidate_identity(identity)
            })
            .transpose()?;
        if let Some(token) = bootstrap {
            SetupRecovery::new(
                self.inner.data_root.clone(),
                self.inner.management_root.clone(),
                BTreeMap::new(),
            )?
            .authorize_bootstrap(token)?;
        } else if identity.is_none() {
            return Err(ApiError::unauthorized());
        }
        let mut setup = SetupRecovery::new(
            self.inner.data_root.clone(),
            self.inner.management_root.clone(),
            self.inner.setup_disks.clone(),
        )?
        .with_presets(self.inner.setup_configs.clone())
        .with_workspace_limits(self.inner.retained_workspace_limits)?;
        if self.inner.allow_development_setup {
            setup = setup.allow_development_mode();
        }
        let permit = bootstrap
            .map(|token| setup.authorize_bootstrap(token))
            .transpose()?;
        if action == "status" {
            return json_codec::to_value(setup.status()).map_err(ApiError::invalid);
        }
        if action == "preset_preview" {
            let request: PresetPreviewRequest =
                json_codec::from_value(input).map_err(ApiError::invalid)?;
            return setup
                .preview_preset(permit.as_ref().ok_or_else(ApiError::unauthorized)?, request);
        }
        // Reject malformed DTOs before changing the optional engine lifecycle.
        match action {
            "initialize" => {
                json_codec::from_value::<SetupRequest>(input.clone()).map_err(ApiError::invalid)?;
            }
            "initialize_preset" => {
                json_codec::from_value::<PresetSetupRequest>(input.clone())
                    .map_err(ApiError::invalid)?;
            }
            "complete_initialization" => {
                json_codec::from_value::<CompleteSetupRequest>(input.clone())
                    .map_err(ApiError::invalid)?;
            }
            "restore_system" => {
                json_codec::from_value::<RestoreRequest>(input.clone())
                    .map_err(ApiError::invalid)?;
            }
            "recovery_preview" => {
                json_codec::from_value::<RecoveryPreviewRequest>(input.clone())
                    .map_err(ApiError::invalid)?;
            }
            "recovery_commit" => {
                json_codec::from_value::<RecoveryCommitRequest>(input.clone())
                    .map_err(ApiError::invalid)?;
            }
            "offline_check" => {
                if input.as_object().is_none_or(|object| {
                    object.len() != 1 || object.get("disk_id").is_none_or(|id| !id.is_string())
                }) {
                    return Err(ApiError::invalid("offline check accepts only disk_id"));
                }
            }
            _ => return Err(ApiError::unsupported()),
        }
        if self.inner.recovering.swap(true, Ordering::SeqCst) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::StorageRetryRunning,
                "another storage lifecycle operation is running",
            ));
        }
        let lifecycle = RecoveryPermit(&self.inner.recovering);
        {
            let mut state = crate::lock(&self.inner.state)?;
            if let Some(backend) = &state.backend {
                if state.effective_status() == StorageState::Ready {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::StorageActive,
                        "offline management requires a faulted storage backend",
                    ));
                }
                let busy = Arc::strong_count(backend) > 1;
                state.admitted = false;
                if busy {
                    let mut error = ApiError::conflict(
                        storage_contract::ErrorCode::StorageDraining,
                        "existing storage operations must finish before offline recovery; retry after they drain",
                    );
                    error.retryable = true;
                    return Err(error);
                }
                state.backend = None;
                state.status = StorageState::RecoveryRequired;
            }
        }
        let install = |engine: storage_engine::Engine, username: &str, password: &str| {
            claim_authority(&self.inner.data_root)?;
            let new_auth =
                AuthStore::initialize_management(&self.inner.management_root, username, password)?;
            let mut snapshot = new_auth.snapshot()?;
            snapshot["sessions"] = json!({});
            *crate::lock(&auth)? = new_auth;
            EngineBackend::new(engine).replicate_auth(snapshot)
        };
        let result = match action {
            "initialize" => {
                let request: SetupRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid)?;
                let permit = permit.as_ref().ok_or_else(ApiError::unauthorized)?;
                setup
                    .initialize(permit, request, install)
                    .map(|_| json!({"initialized":true,"restart_required":false,"recovering":true}))
            }
            "initialize_preset" => {
                let request: PresetSetupRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid)?;
                let permit = permit.as_ref().ok_or_else(ApiError::unauthorized)?;
                setup
                    .initialize_preset(permit, request, install)
                    .map(|_| json!({"initialized":true,"restart_required":false,"recovering":true}))
            }
            "complete_initialization" => {
                let request: CompleteSetupRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid)?;
                let permit = permit.as_ref().ok_or_else(ApiError::unauthorized)?;
                setup.complete_initialization(permit,request,install)
                    .map(|_|json!({"initialized":true,"completed":true,"restart_required":false,"recovering":true}))
            }
            "restore_system" => {
                let request: RestoreRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid)?;
                let restore = |engine: storage_engine::Engine| {
                    if let Some(identity) = &identity {
                        // Disk reconstruction can outlive the original session.
                        // Recheck immediately before publishing system authority.
                        crate::lock(&auth)?.revalidate_identity(identity)?;
                    }
                    claim_authority(&self.inner.data_root)?;
                    // Valid local system authority wins over an older DR copy.
                    let snapshot = if crate::lock(&auth)?.is_available() {
                        crate::lock(&auth)?.snapshot()?
                    } else {
                        let mut snapshot = engine.auth_state().ok_or_else(|| {
                            ApiError::new(
                                503,
                                storage_contract::ErrorCode::AuthRecoveryRequired,
                                "restored storage contains no credential backup",
                            )
                        })?;
                        snapshot["sessions"] = json!({});
                        let restored = AuthStore::restore_management(
                            &self.inner.management_root,
                            snapshot.clone(),
                        )?;
                        *crate::lock(&auth)? = restored;
                        snapshot
                    };
                    let mut snapshot = snapshot;
                    snapshot["sessions"] = json!({});
                    EngineBackend::new(engine).replicate_auth(snapshot)
                };
                if let Some(identity) = &identity {
                    setup.restore_authenticated(identity, request, restore)
                } else {
                    setup.restore_system(
                        permit.as_ref().ok_or_else(ApiError::unauthorized)?,
                        request,
                        restore,
                    )
                }
                .map(|_| json!({"restored":true,"restart_required":false,"recovering":true}))
            }
            "offline_check" => {
                if input
                    .as_object()
                    .is_none_or(|object| object.len() != 1 || !object.contains_key("disk_id"))
                {
                    return Err(ApiError::invalid("offline check accepts only disk_id"));
                }
                let identity = identity.as_ref().ok_or_else(ApiError::unauthorized)?;
                let disk = input
                    .get("disk_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::invalid("disk_id required"))?;
                setup
                    .offline_check(identity, disk)
                    .and_then(|value| json_codec::to_value(value).map_err(ApiError::invalid))
            }
            "recovery_preview" => {
                let request: RecoveryPreviewRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid)?;
                setup
                    .recovery_preview(
                        identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                        request,
                        |identity| {
                            crate::lock(&auth)?
                                .revalidate_identity(identity)
                                .map(|_| ())
                        },
                    )
                    .and_then(|value| json_codec::to_value(value).map_err(ApiError::invalid))
            }
            "recovery_commit" => {
                let request: RecoveryCommitRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid)?;
                setup
                    .recovery_commit(
                        identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                        request,
                        |identity| {
                            crate::lock(&auth)?
                                .revalidate_identity(identity)
                                .map(|_| ())
                        },
                    )
                    .and_then(|value| json_codec::to_value(value).map_err(ApiError::invalid))
            }
            _ => Err(ApiError::unsupported()),
        };
        drop(lifecycle);
        if result.is_ok()
            && matches!(
                action,
                "initialize"
                    | "initialize_preset"
                    | "complete_initialization"
                    | "restore_system"
                    | "recovery_commit"
            )
        {
            return self.inner.resume_after_setup(result?);
        }
        result
    }

    fn start_management(&self, auth: Arc<Mutex<AuthStore>>) {
        if self.inner.ensure_running().is_err() {
            return;
        }
        if let Ok(mut current) = self.inner.auth.lock() {
            if current.is_some() {
                return;
            }
            *current = Some(auth);
        }
        let _ = self.inner.recover();
        let weak = Arc::downgrade(&self.inner);
        let _ = std::thread::Builder::new()
            .name("zxs-disk-observation".into())
            .spawn(move || {
                loop {
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    if inner.stopping.load(Ordering::SeqCst) {
                        break;
                    }
                    inner.observe_storage_once();
                    drop(inner);
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            });
    }
    fn stop_management(&self) {
        self.inner.stop();
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        self.inner.drain()
    }
    fn management_status(&self) -> Value {
        let state = self
            .inner
            .state
            .lock()
            .map(|s| s.effective_status())
            .unwrap_or(StorageState::RecoveryRequired);
        json!({"state":state.as_str(),"retryable":state!=StorageState::Ready,"recovering":self.inner.recovering.load(Ordering::SeqCst)})
    }
    fn retry_storage(&self, identity: &Identity) -> Result<Value, ApiError> {
        if identity.admission_session.is_none() {
            return Err(ApiError::unauthorized());
        }
        let auth = crate::lock(&self.inner.auth)?
            .clone()
            .ok_or_else(ApiError::unauthorized)?;
        crate::lock(&auth)?.revalidate_identity(identity)?;
        self.inner.start_recovery(true)?;
        Ok(self.management_status())
    }
    fn call(
        &self,
        action: BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        if action == BackendAction::Capabilities {
            let mut value = crate::lock(&self.inner.capabilities)?.clone();
            value["storage"] = self.management_status();
            return Ok(value);
        }
        if action == BackendAction::Ready {
            return if self.management_status()["state"] == "ready" {
                Ok(json!({"ready":true}))
            } else {
                Err(ApiError::new(
                    503,
                    storage_contract::ErrorCode::StorageUnavailable,
                    "storage is not ready",
                ))
            };
        }
        self.inner.backend()?.call(action, identity, input)
    }
    fn upload(
        &self,
        i: &Identity,
        w: &str,
        item: &str,
        path: &str,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.inner.backend()?.upload(i, w, item, path, source)
    }
    fn upload_generation(
        &self,
        i: &Identity,
        w: &str,
        item: &str,
        path: &str,
        source: &crate::UploadSource,
        g: u64,
    ) -> Result<Value, ApiError> {
        self.inner
            .backend()?
            .upload_generation(i, w, item, path, source, g)
    }
    fn content(&self, i: &Identity, v: &str) -> Result<Content, ApiError> {
        self.inner.backend()?.content(i, v)
    }
    fn range_content(&self, i: &Identity, v: &str) -> Result<Content, ApiError> {
        self.inner.backend()?.range_content(i, v)
    }
    fn workspace_content(&self, i: &Identity, w: &str, item: &str) -> Result<Content, ApiError> {
        self.inner.backend()?.workspace_content(i, w, item)
    }
    fn checksums(&self, i: &Identity, v: &str) -> Result<Vec<u8>, ApiError> {
        self.inner.backend()?.checksums(i, v)
    }
    fn observe_disks(&self, _o: &InternalDiskObservation) -> Result<Value, ApiError> {
        // Only a single background observer can block on disk I/O. HTTP host
        // diagnostics read a bounded cache and explicitly reject stale data.
        let cache = crate::lock(&self.inner.disk_observation)?;
        cache
            .as_ref()
            .filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(15))
            .map(|(_, value)| value.clone())
            .ok_or_else(|| {
                ApiError::new(
                    503,
                    storage_contract::ErrorCode::DiskSampleUnavailable,
                    "disk observations are unavailable or stale",
                )
            })
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/management.rs"]
mod management_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/management_authority_tests.rs"]
mod authority_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/management_recovery_admission_tests.rs"]
mod recovery_admission_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/management_shutdown_tests.rs"]
mod shutdown_tests;
