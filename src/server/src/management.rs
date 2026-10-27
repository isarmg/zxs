//! System-disk control plane. Engine recovery is optional and never gates HTTP.
//! `management-auth.json` is primary; engine snapshots are disaster-recovery
//! copies. Only explicit authorized recovery can install them; background
//! recovery and restarts never replace missing or damaged system credentials.
use crate::{
    ApiError, Backend, Content, InternalDiskObservation,
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

pub use crate::StorageState;
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
    disk_observation: Mutex<Option<(std::time::Instant, Vec<crate::DiskSample>)>>,
    capabilities: Mutex<crate::Capabilities>,
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
        limits.validate().map_err(ApiError::invalid_source)?;
    }
    // These are system state directories, never data-disk mountpoints.
    if !setup_disks.is_empty() || !setup_configs.is_empty() {
        create_private_directory(data_root)?;
    }
    create_private_directory(management_root)?;
    let mut auth = AuthStore::open_management(management_root).unwrap_or_else(|error| {
        tracing::warn!(code = %error.code, "management credentials require recovery");
        AuthStore::unavailable(management_root)
    });
    if auth.is_available() {
        // Record local authority before accepting any credential mutation,
        // including when the data pool itself cannot be opened.
        let claim = create_private_directory(data_root).and_then(|_| claim_authority(data_root));
        if let Err(error) = claim {
            tracing::warn!(code = %error.code,"cannot durably establish management authority");
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
            capabilities: Mutex::new(crate::Capabilities::unavailable()),
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
#[derive(Debug, error_definition::Error)]
#[error("storage recovery worker panicked: {message:?}")]
struct RecoveryWorkerPanic {
    message: Option<String>,
}

#[derive(Debug, error_definition::Error)]
#[error(
    "management shutdown failed (recovery worker poisoned: {recovery_worker_poisoned}, admission state poisoned: {admission_state_poisoned})"
)]
struct RecoveryShutdownError {
    recovery_worker_poisoned: bool,
    admission_state_poisoned: bool,
    #[source]
    worker_panic: Option<RecoveryWorkerPanic>,
}

struct RecoveryPermit<'a>(&'a AtomicBool);
impl Drop for RecoveryPermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
impl Inner {
    fn set(&self, backend: Option<Arc<EngineBackend>>, status: StorageState) {
        {
            let mut state = self.state.lock().expect("service state lock poisoned");
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
                tracing::warn!(error = ?error, "completed setup requires a service restart");
                completed["recovering"] = json!(false);
                completed["restart_required"] = json!(true);
                Ok(completed)
            }
        }
    }
    fn start_recovery(self: &Arc<Self>, reopen: bool) -> Result<(), ApiError> {
        // Serialize enqueue with stop, including ownership of a newly spawned
        // native thread. No thread may escape the shutdown owner's drain.
        let mut worker = crate::lock(&self.recovery_worker);
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
            previous
                .join()
                .expect("invariant: storage recovery worker does not panic");
        }
        if reopen {
            let mut state = crate::lock(&self.state);
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
        let mut worker = crate::lock(&self.recovery_worker);
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
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::ServiceStopping,
                "service is stopping",
            ))
        } else {
            Ok(())
        }
    }
    fn stop(&self) {
        // Only shutdown consumes poisoned guards: fence admission while still
        // serialized with enqueue, never clear poison or resume business use.
        let worker = match self.recovery_worker.lock() {
            Ok(worker) => worker,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.stopping.store(true, Ordering::SeqCst);
        drop(worker);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.admitted = false;
        state.status = StorageState::Unavailable;
    }
    fn drain(&self) -> Result<(), ApiError> {
        self.stop();
        let worker = match self.recovery_worker.lock() {
            Ok(mut worker) => worker.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        // No worker/admission lock is held while joining. A panicked recovery
        // thread remains a concrete shutdown failure, not a second panic from
        // an owner's destructor or a claim that recovery completed cleanly.
        let worker_panic =
            worker
                .and_then(|worker| worker.join().err())
                .map(|panic| RecoveryWorkerPanic {
                    message: panic.downcast_ref::<String>().cloned().or_else(|| {
                        panic
                            .downcast_ref::<&str>()
                            .map(|message| (*message).to_owned())
                    }),
                });
        let failure = RecoveryShutdownError {
            recovery_worker_poisoned: self.recovery_worker.is_poisoned(),
            admission_state_poisoned: self.state.is_poisoned(),
            worker_panic,
        };
        if failure.recovery_worker_poisoned
            || failure.admission_state_poisoned
            || failure.worker_panic.is_some()
        {
            return Err(ApiError::io(std::io::Error::other(failure)));
        }
        Ok(())
    }
    fn run_recovery(self: Arc<Self>, mut recover: impl FnMut(&Arc<Self>) -> Result<(), ApiError>) {
        loop {
            let generation = self.generation.load(Ordering::SeqCst);
            if let Err(error) = recover(&self) {
                tracing::warn!(error = ?error,"storage unavailable; management remains online");
                {
                    let mut state = self.state.lock().expect("service state lock poisoned");
                    state.admitted = false;
                    state.status = StorageState::RecoveryRequired;
                }
            }
            // Coalesce auth changes in this owned thread instead of recursively
            // spawning a replacement that the shutdown owner might miss.
            let _worker = self
                .recovery_worker
                .lock()
                .expect("service state lock poisoned");
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
        let existing = crate::lock(&self.state).backend.clone();
        let backend = if let Some(backend) = existing {
            backend
        } else {
            let engine = match storage_engine::Engine::open_with_workspace_limits(
                &self.data_root,
                self.retained_workspace_limits,
            ) {
                Ok(engine) => engine,
                Err(error) => {
                    tracing::warn!(error = ?error,"storage recovery failed");
                    let status = if !self
                        .data_root
                        .join("config.json")
                        .try_exists()
                        .map_err(ApiError::io)?
                    {
                        if self
                            .data_root
                            .join("management-authority.json")
                            .try_exists()
                            .map_err(ApiError::io)?
                        {
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
        let auth = crate::lock(&self.auth).clone().ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::AuthRecoveryRequired,
                "management is not initialized",
            )
        })?;
        {
            let mut auth = crate::lock(&auth);
            // A recovered pool is evidence, never permission to install system
            // credentials. In particular, disk restore may have completed before
            // bootstrap authorization expired or was revoked. Neither an absent
            // handover marker nor a restart may turn that rejection into consent.
            if !auth.is_available() {
                return Err(ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
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
                let mut last = crate::lock(&last);
                if *last == snapshot {
                    return Ok(());
                }
                *last = snapshot;
                if let Some(inner) = weak.upgrade() {
                    // Revoke data admission synchronously. Only the bounded,
                    // coalescing worker touches the engine/data disks.
                    {
                        let mut state = crate::lock(&inner.state);
                        state.admitted = false;
                        state.status = StorageState::RecoveryRequired;
                    }
                    inner.generation.fetch_add(1, Ordering::SeqCst);
                    match inner.recover() {
                        Ok(()) => {}
                        Err(error)
                            if matches!(
                                error.code,
                                storage_contract::ErrorCode::StorageRetryRunning
                                    | storage_contract::ErrorCode::ServiceStopping
                            ) => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            }));
        }
        backend.set_management_auth(crate::lock(&auth).reader())?;
        self.set(Some(backend.clone()), StorageState::RecoveryRequired);
        loop {
            let mut snapshot = crate::lock(&auth).snapshot()?;
            snapshot["sessions"] = json!({});
            backend.replicate_auth(snapshot.clone())?;
            let capabilities = backend.capabilities()?;
            *crate::lock(&self.capabilities) = capabilities;
            let ready = if backend.read_only()? {
                StorageState::ReadOnly
            } else {
                StorageState::Ready
            };
            let current = crate::lock(&auth);
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
        let state = crate::lock(&self.state);
        state.backend.as_ref().filter(|_|state.admitted && state.status!=StorageState::Unavailable && state.observed_at.elapsed()<std::time::Duration::from_secs(15)).cloned().ok_or_else(||ApiError::new(::http::StatusCode::SERVICE_UNAVAILABLE,storage_contract::ErrorCode::StorageUnavailable,"storage is unavailable; inspect /api/system/storage and retry after correcting the fault"))
    }
    /// Keep the engine lease inside one sample. The observer's idle interval
    /// must not look like an in-flight storage operation to recovery admission.
    fn observe_storage_once(&self) {
        let backend = {
            let state = crate::lock(&self.state);
            state.backend.as_ref().filter(|_| state.admitted).cloned()
        };
        let Some(backend) = backend.as_ref() else {
            *crate::lock(&self.disk_observation) = None;
            return;
        };
        let observation = {
            let health = backend.ready();
            let mut state = crate::lock(&self.state);
            if state.admitted
                && state
                    .backend
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, backend))
            {
                state.observed_at = std::time::Instant::now();
                let previous = state.status;
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
                if state.status != previous
                    && let Err(error) = &health
                {
                    tracing::warn!(error = ?error, "storage readiness changed after failed health observation");
                }
            }
            drop(state);
            backend.observe_disks(&InternalDiskObservation(()))
        };
        {
            let mut cache = self
                .disk_observation
                .lock()
                .expect("service state lock poisoned");
            *cache = match observation {
                Ok(value) => Some((std::time::Instant::now(), value)),
                Err(error) => {
                    tracing::warn!(error = ?error, "disk observation unavailable; cached observation cleared");
                    None
                }
            };
        }
    }
}
impl Backend for ManagementBackend {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.inner.backend()?.operation(identity, operation_id)
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.inner.backend()?.commit_workspace(identity, command)
    }

    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError> {
        self.inner.backend()?.snapshot_at(identity, branch, commit)
    }

    fn create_workspace(
        &self,
        identity: &Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, ApiError> {
        self.inner.backend()?.create_workspace(identity, command)
    }
    fn abort_workspace(&self, identity: &Identity, workspace: &str) -> Result<(), ApiError> {
        self.inner.backend()?.abort_workspace(identity, workspace)
    }

    fn workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.inner.backend()?.workspace(identity, workspace)
    }
    fn published_upload_status(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        self.inner
            .backend()?
            .published_upload_status(identity, workspace, item)
    }
    fn seal_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        self.inner.backend()?.seal_workspace(identity, workspace)
    }
    fn commit_workspace_public(
        &self,
        identity: &crate::auth::Identity,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .commit_workspace_public(identity, command)
    }
    fn version_snapshot(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        self.inner.backend()?.version_snapshot(identity, version)
    }
    fn authorize_version(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<(), crate::ApiError> {
        self.inner.backend()?.authorize_version(identity, version)
    }
    fn repository_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.inner.backend()?.repository_snapshot(identity, id)
    }
    fn authorize_repository(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<(), crate::ApiError> {
        self.inner.backend()?.authorize_repository(identity, id)
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        let capabilities = crate::lock(&self.inner.capabilities).clone();
        Ok(capabilities.with_storage(self.management_status()))
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        if self.management_status().state() == StorageState::Ready {
            Ok(crate::Readiness::Managed)
        } else {
            Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::StorageUnavailable,
                "storage is not ready",
            ))
        }
    }
    fn workspace_capacity(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        self.inner.backend()?.workspace_capacity(identity)
    }
    fn smb_binding_status(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        self.inner.backend()?.smb_binding_status(identity, id)
    }
    fn update_smb_binding(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.update_smb_binding(identity, request)
    }
    fn protection_policy(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        self.inner.backend()?.protection_policy(identity, id)
    }
    fn placement_rule(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        self.inner.backend()?.placement_rule(identity, id)
    }
    fn update_protection_policy(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .update_protection_policy(identity, request)
    }
    fn update_placement_rule(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .update_placement_rule(identity, request)
    }
    fn compare_commits(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        self.inner.backend()?.compare_commits(identity, request)
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        self.inner.backend()?.disk_status(identity)
    }
    fn integrity_check(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        self.inner.backend()?.integrity_check(identity)
    }
    fn create_merge_link(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        self.inner.backend()?.create_merge_link(identity, request)
    }
    fn create_repository(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.inner.backend()?.create_repository(identity, request)
    }
    fn create_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.inner.backend()?.create_branch(identity, request)
    }
    fn create_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.create_file_branch(identity, request)
    }
    fn update_note(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.update_note(identity, request)
    }
    fn merge_file_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.merge_file_branches(identity, request)
    }
    fn update_file_node_tags(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .update_file_node_tags(identity, request)
    }
    fn create_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .create_disk_directory(identity, request)
    }
    fn create_disk_write_session(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        self.inner
            .backend()?
            .create_disk_write_session(identity, request)
    }
    fn backup_configuration(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
        self.inner
            .backend()?
            .backup_configuration(identity, request)
    }
    fn configure_backups(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.configure_backups(identity, request)
    }
    fn plan(
        &self,
        identity: &crate::auth::Identity,
        plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner.backend()?.plan(identity, plan)
    }
    fn cancel_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.cancel_plan(identity, request)
    }
    fn register_disk(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        self.inner.backend()?.register_disk(identity)
    }
    fn commit_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.commit_plan(identity, request)
    }
    fn prepare_historical_save(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner
            .backend()?
            .prepare_historical_save(identity, request)
    }
    fn prepare_purge(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner.backend()?.prepare_purge(identity, request)
    }
    fn prepare_split(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner.backend()?.prepare_split(identity, request)
    }
    fn namespace_batch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.namespace_batch(identity, request)
    }
    fn select_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.select_file_branch(identity, request)
    }
    fn rename_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.rename_file_branch(identity, request)
    }
    fn listing(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        self.inner.backend()?.listing(identity, request)
    }
    fn mutate_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .mutate_disk_directory(identity, request)
    }
    fn initialize_directory_scope(
        &self,
        identity: &crate::auth::Identity,
        request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .initialize_directory_scope(identity, request)
    }
    fn archive_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.archive_branch(identity, request)
    }
    fn rename_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .rename_directory_branch(identity, request)
    }
    fn select_default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .backend()?
            .select_default_directory_branch(identity, request)
    }
    fn directory_selection(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        self.inner.backend()?.directory_selection(identity, request)
    }
    fn directory_scopes(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        self.inner.backend()?.directory_scopes(identity, request)
    }
    fn default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        self.inner
            .backend()?
            .default_directory_branch(identity, repository_id)
    }
    fn file_history_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistorySnapshot, crate::ApiError> {
        self.inner
            .backend()?
            .file_history_snapshot(identity, request)
    }
    fn file_history(
        &self,
        identity: &crate::auth::Identity,
        file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        self.inner.backend()?.file_history(identity, file)
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        self.inner.backend()?.topology_snapshot(identity)
    }
    fn placement_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        self.inner.backend()?.placement_snapshot(identity, id)
    }
    fn event_page(
        &self,
        identity: &crate::auth::Identity,
        after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        self.inner.backend()?.event_page(identity, after)
    }
    fn activity_history(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        self.inner.backend()?.activity_history(identity)
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        self.inner.backend()?.history_graph(identity, id)
    }
    fn commit_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.inner.backend()?.commit_snapshot(identity, id)
    }
    fn resolve_entry(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        self.inner.backend()?.resolve_entry(identity, request)
    }
    fn branch_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.inner.backend()?.branch_snapshot(identity, id)
    }
    fn branches_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        self.inner.backend()?.branches_snapshot(identity, id)
    }
    fn repositories_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        self.inner.backend()?.repositories_snapshot(identity)
    }
    fn tree_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.inner.backend()?.tree_snapshot(identity, request)
    }
    fn create_write_session(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        self.inner.backend()?.create_write_session(identity, branch)
    }
    fn stage_namespace(
        &self,
        identity: &crate::auth::Identity,
        command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.inner.backend()?.stage_namespace(identity, command)
    }
    fn authorize_upload(
        &self,
        identity: &Identity,
        workspace: &str,
        logical_size: Option<u64>,
    ) -> Result<(), ApiError> {
        self.inner
            .backend()?
            .authorize_upload(identity, workspace, logical_size)
    }

    fn read_directory(
        &self,
        identity: &Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, ApiError> {
        self.inner.backend()?.read_directory(identity, request)
    }

    fn authorize_workspace_selection(
        &self,
        identity: &Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), ApiError> {
        self.inner
            .backend()?
            .authorize_workspace_selection(identity, workspace, path)
    }

    fn authorize_current(&self, identity: &Identity, branch: &str) -> Result<(), ApiError> {
        self.inner.backend()?.authorize_current(identity, branch)
    }

    fn lookup_current(
        &self,
        identity: &Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        self.inner.backend()?.lookup_current(identity, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        self.inner
            .backend()?
            .lookup_at(identity, branch, commit, path)
    }

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
        let auth = crate::lock(&self.inner.auth)
            .clone()
            .ok_or_else(ApiError::unauthorized)?;
        let identity = identity
            .map(|identity| {
                // Recovery is an HTTP management operation, never an SMB
                // principal-only operation. Require its current login session.
                if identity.admission_session.is_none() {
                    return Err(ApiError::unauthorized());
                }
                crate::lock(&auth).revalidate_identity(identity)
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
            return Ok(json_codec::to_value(setup.status())
                .expect("invariant: setup status serialization succeeds"));
        }
        if action == "preset_preview" {
            let request: PresetPreviewRequest =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            return setup
                .preview_preset(permit.as_ref().ok_or_else(ApiError::unauthorized)?, request);
        }
        // Reject malformed DTOs before changing the optional engine lifecycle.
        let mut recovery_preview = None;
        match action {
            "initialize" => {
                json_codec::from_value::<SetupRequest>(input.clone())
                    .map_err(ApiError::invalid_source)?;
            }
            "initialize_preset" => {
                json_codec::from_value::<PresetSetupRequest>(input.clone())
                    .map_err(ApiError::invalid_source)?;
            }
            "complete_initialization" => {
                json_codec::from_value::<CompleteSetupRequest>(input.clone())
                    .map_err(ApiError::invalid_source)?;
            }
            "restore_system" => {
                json_codec::from_value::<RestoreRequest>(input.clone())
                    .map_err(ApiError::invalid_source)?;
            }
            "recovery_preview" => {
                recovery_preview = Some(RecoveryPreviewRequest::parse(input.clone())?);
            }
            "recovery_commit" => {
                json_codec::from_value::<RecoveryCommitRequest>(input.clone())
                    .map_err(ApiError::invalid_source)?;
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
            let mut state = crate::lock(&self.inner.state);
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
            crate::lock(&auth).replace_authority(new_auth);
            EngineBackend::new(engine).replicate_auth(snapshot)
        };
        let result = match action {
            "initialize" => {
                let request: SetupRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let permit = permit.as_ref().ok_or_else(ApiError::unauthorized)?;
                setup
                    .initialize(permit, request, install)
                    .map(|_| json!({"initialized":true,"restart_required":false,"recovering":true}))
            }
            "initialize_preset" => {
                let request: PresetSetupRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let permit = permit.as_ref().ok_or_else(ApiError::unauthorized)?;
                setup
                    .initialize_preset(permit, request, install)
                    .map(|_| json!({"initialized":true,"restart_required":false,"recovering":true}))
            }
            "complete_initialization" => {
                let request: CompleteSetupRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let permit = permit.as_ref().ok_or_else(ApiError::unauthorized)?;
                setup.complete_initialization(permit,request,install)
                    .map(|_|json!({"initialized":true,"completed":true,"restart_required":false,"recovering":true}))
            }
            "restore_system" => {
                let request: RestoreRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let restore = |engine: storage_engine::Engine| {
                    if let Some(identity) = &identity {
                        // Disk reconstruction can outlive the original session.
                        // Recheck immediately before publishing system authority.
                        crate::lock(&auth).revalidate_identity(identity)?;
                    }
                    claim_authority(&self.inner.data_root)?;
                    // Valid local system authority wins over an older DR copy.
                    let snapshot = if crate::lock(&auth).is_available() {
                        crate::lock(&auth).snapshot()?
                    } else {
                        let mut snapshot = engine.auth_state().ok_or_else(|| {
                            ApiError::new(
                                ::http::StatusCode::SERVICE_UNAVAILABLE,
                                storage_contract::ErrorCode::AuthRecoveryRequired,
                                "restored storage contains no credential backup",
                            )
                        })?;
                        snapshot["sessions"] = json!({});
                        let restored = AuthStore::restore_management(
                            &self.inner.management_root,
                            snapshot.clone(),
                        )?;
                        crate::lock(&auth).replace_authority(restored);
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
                setup.offline_check(identity, disk).map(|value| {
                    json_codec::to_value(value)
                        .expect("invariant: internal response serialization succeeds")
                })
            }
            "recovery_preview" => {
                let request = recovery_preview
                    .expect("invariant: recovery preview was parsed before lifecycle changes");
                setup
                    .recovery_preview(
                        identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                        request,
                        |identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ()),
                    )
                    .map(|value| {
                        json_codec::to_value(value)
                            .expect("invariant: internal response serialization succeeds")
                    })
            }
            "recovery_commit" => {
                let request: RecoveryCommitRequest =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                setup
                    .recovery_commit(
                        identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                        request,
                        |identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ()),
                    )
                    .map(|value| {
                        json_codec::to_value(value)
                            .expect("invariant: internal response serialization succeeds")
                    })
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
        {
            let mut current = self.inner.auth.lock().expect("service state lock poisoned");
            if current.is_some() {
                return;
            }
            *current = Some(auth);
        }
        if let Err(error) = self.inner.recover() {
            tracing::warn!(error = ?error, "initial storage recovery could not start");
        }
        let weak = Arc::downgrade(&self.inner);
        if let Err(error) = std::thread::Builder::new()
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
            })
        {
            tracing::warn!(error = ?error, "disk observation worker could not start");
        }
    }
    fn stop_management(&self) {
        self.inner.stop();
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        self.inner.drain()
    }
    fn management_status(&self) -> crate::StorageStatus {
        let state = self
            .inner
            .state
            .lock()
            .expect("service state lock poisoned")
            .effective_status();
        crate::StorageStatus::Managed {
            state,
            recovering: self.inner.recovering.load(Ordering::SeqCst),
        }
    }
    fn retry_storage(&self, identity: &Identity) -> Result<crate::StorageStatus, ApiError> {
        if identity.admission_session.is_none() {
            return Err(ApiError::unauthorized());
        }
        let auth = crate::lock(&self.inner.auth)
            .clone()
            .ok_or_else(ApiError::unauthorized)?;
        crate::lock(&auth).revalidate_identity(identity)?;
        self.inner.start_recovery(true)?;
        Ok(self.management_status())
    }
    fn upload(
        &self,
        i: &Identity,
        w: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.inner.backend()?.upload(i, w, item, path, source)
    }
    fn upload_generation(
        &self,
        i: &Identity,
        w: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        g: u64,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.inner
            .backend()?
            .upload_generation(i, w, item, path, source, g)
    }
    fn content(&self, i: &Identity, v: &str) -> Result<Content, ApiError> {
        self.inner.backend()?.content(i, v)
    }
    fn range_content(
        &self,
        i: &Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<Content, ApiError> {
        self.inner.backend()?.range_content(i, request)
    }
    fn workspace_content(&self, i: &Identity, w: &str, item: &str) -> Result<Content, ApiError> {
        self.inner.backend()?.workspace_content(i, w, item)
    }
    fn checksums(&self, i: &Identity, v: &str) -> Result<Vec<u8>, ApiError> {
        self.inner.backend()?.checksums(i, v)
    }
    fn observe_disks(
        &self,
        _o: &InternalDiskObservation,
    ) -> Result<Vec<crate::DiskSample>, ApiError> {
        // Only a single background observer can block on disk I/O. HTTP host
        // diagnostics read a bounded cache and explicitly reject stale data.
        let cache = crate::lock(&self.inner.disk_observation);
        cache
            .as_ref()
            .filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(15))
            .map(|(_, value)| value.clone())
            .ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
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
