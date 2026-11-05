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
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

mod disk_notes;
mod terminal_backend;
pub(crate) mod terminal_resume;
use terminal_backend::BackendState;
mod integrity_scrub;
pub(crate) mod storage_addition;
pub(crate) mod storage_configuration;
pub(crate) mod storage_lifecycle;
pub(crate) mod storage_readmission;
use integrity_scrub::IntegrityScrubService;
mod setup_command;
use setup_command::SetupCommand;
mod startup_diagnostic;
use startup_diagnostic::StartupDiagnostic;

pub use crate::StorageState;
struct State {
    backend: BackendState,
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
    disk_notes: Mutex<()>,
    storage_addition: Mutex<()>,
    capabilities: Mutex<crate::Capabilities>,
    setup_disks: BTreeMap<String, PathBuf>,
    setup_configs: BTreeMap<String, PathBuf>,
    allow_development_setup: bool,
    retained_workspace_limits: Option<storage_engine::RetainedWorkspaceLimits>,
}
pub struct ManagementBackend {
    // Fields drop in declaration order: join scrub before releasing Inner/auth.
    scrub: IntegrityScrubService,
    observer: crate::periodic_worker::PeriodicWorker,
    drain: Mutex<()>,
    inner: Arc<Inner>,
}
/// Locally provisioned setup choices and immutable engine policy. Paths are
/// retained without reading them; unavailable presets never gate management startup.
#[derive(Default)]
pub struct ManagementOptions {
    /// Immutable host scheduling and bandwidth policy for full integrity jobs.
    pub scrub_policy: crate::integrity_scrub::IntegrityScrubPolicy,
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
    options.write(true).create_new(true).mode(0o600);
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
            if metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
                return Err(ApiError::invalid("unsafe management authority marker"));
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
            scrub_policy: crate::integrity_scrub::IntegrityScrubPolicy::default(),
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
        scrub_policy,
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
    create_private_directory(data_root)?;
    create_private_directory(management_root)?;
    let mut auth = AuthStore::open_management(management_root).unwrap_or_else(|error| {
        tracing::warn!(code = %error.code, error = %StartupDiagnostic(&error), "management credentials require recovery");
        AuthStore::unavailable()
    });
    if auth.is_available() {
        // Record local authority before accepting any credential mutation,
        // including when the data pool itself cannot be opened.
        let claim = create_private_directory(data_root).and_then(|_| claim_authority(data_root));
        if let Err(error) = claim {
            tracing::warn!(code = %error.code, error = %StartupDiagnostic(&error), "cannot durably establish management authority");
            auth = AuthStore::unavailable();
        }
    }
    let backend = Arc::new(ManagementBackend {
        scrub: IntegrityScrubService::new(scrub_policy),
        observer: crate::periodic_worker::PeriodicWorker::new(),
        drain: Mutex::new(()),
        inner: Arc::new(Inner {
            data_root: data_root.into(),
            management_root: management_root.into(),
            state: Mutex::new(State {
                backend: BackendState::Unopened,
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
            disk_notes: Mutex::new(()),
            storage_addition: Mutex::new(()),
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
    builder.recursive(true).mode(0o700);
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

#[derive(Debug, error_definition::Error)]
#[error("management recovery drain failed: {recovery}; integrity scrub drain failed: {scrub}")]
struct ManagementDrainErrors {
    #[source]
    recovery: ApiError,
    scrub: ApiError,
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
            if matches!(
                state.backend,
                BackendState::Draining(_)
                    | BackendState::ChangingAuthority
                    | BackendState::Terminal(_)
                    | BackendState::MetadataResume(_)
                    | BackendState::ResumeRecovery
            ) {
                // A recovery worker admitted before the exit cannot restore a
                // dispatcher after terminal handover has frozen admission.
                return;
            }
            state.backend = backend.map_or(BackendState::Unopened, BackendState::Engine);
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
            if matches!(
                state.backend,
                BackendState::Draining(_)
                    | BackendState::ChangingAuthority
                    | BackendState::Terminal(_)
                    | BackendState::MetadataResume(_)
                    | BackendState::ResumeRecovery
            ) {
                self.recovering.store(false, Ordering::SeqCst);
                return Ok(());
            }
            state.status = state.effective_status();
            if state.status != StorageState::Ready {
                state.admitted = false;
                if state
                    .backend
                    .engine()
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
                state.backend = BackendState::Unopened;
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
        let _addition = self.storage_addition.lock().map_err(|_| {
            ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "storage addition coordinator failed during shutdown",
            )
        })?;
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
        if self.recover_terminal()? {
            return Ok(());
        }
        let existing = crate::lock(&self.state).backend.engine().cloned();
        let backend = if let Some(backend) = existing {
            backend
        } else {
            let setup = crate::setup_recovery::SetupRecovery::new(
                self.data_root.clone(),
                self.management_root.clone(),
                BTreeMap::new(),
            )?
            .with_workspace_limits(self.retained_workspace_limits)?;
            setup.activation_admitted()?;
            if setup.storage_status().state
                == crate::setup_recovery::StorageActivationState::Pending
            {
                self.set(None, StorageState::Unconfigured);
                return Ok(());
            }
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
                        if matches!(
                            state.backend,
                            BackendState::Draining(_)
                                | BackendState::ChangingAuthority
                                | BackendState::Terminal(_)
                                | BackendState::MetadataResume(_)
                                | BackendState::ResumeRecovery
                        ) {
                            // Current system credentials remain authoritative;
                            // terminal historical evidence is never rewritten.
                            return Ok(());
                        }
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
        state.backend.active_engine().filter(|_|state.admitted && state.status!=StorageState::Unavailable && state.observed_at.elapsed()<std::time::Duration::from_secs(15)).cloned().ok_or_else(||ApiError::new(::http::StatusCode::SERVICE_UNAVAILABLE,storage_contract::ErrorCode::StorageUnavailable,"storage is unavailable; inspect /api/system/storage and retry after correcting the fault"))
    }
    /// Keep the engine lease inside one sample. The observer's idle interval
    /// must not look like an in-flight storage operation to recovery admission.
    fn observe_storage_once(&self) {
        let backend = {
            let state = crate::lock(&self.state);
            state
                .backend
                .active_engine()
                .filter(|_| state.admitted)
                .cloned()
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
                    .engine()
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
/// Simulated registration directories are not evidence of physical device ownership.
fn disk_hardware_root<'a>(mode: &storage_contract::Mode, root: &'a Path) -> Option<&'a Path> {
    match mode {
        storage_contract::Mode::Development => None,
        storage_contract::Mode::Production => Some(root),
    }
}

impl Backend for ManagementBackend {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, ApiError> {
        if let Some(outcome) = self.inner.terminal_operation(identity, operation_id)? {
            return Ok(crate::OperationObservation {
                outcome,
                health: None,
            });
        }
        self.inner.backend()?.operation(identity, operation_id)
    }
    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationOutcome, ApiError> {
        if let Some(outcome) = self.inner.terminal_operation(identity, operation_id)? {
            return Ok(outcome);
        }
        self.inner
            .backend()?
            .operation_outcome(identity, operation_id)
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
    fn require_native_workspace_commit_completion(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<(), ApiError> {
        self.inner
            .backend()?
            .require_native_workspace_commit_completion(identity, command)
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
            Ok(crate::Readiness::Managed {
                history_index: self.inner.backend()?.history_index_status(),
            })
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
    fn resolve_smb_path(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveSmbPath,
    ) -> Result<crate::SmbPathLocation, crate::ApiError> {
        self.inner.backend()?.resolve_smb_path(identity, request)
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
        if let Some(disks) = self.inner.terminal_disk_status(identity)? {
            return Ok(disks);
        }
        self.inner.backend()?.disk_status(identity)
    }
    fn disk_details(
        &self,
        identity: &Identity,
        slot: &str,
    ) -> Result<storage_contract::disk_details::DiskDetailsRecord, ApiError> {
        if let Some(details) = self.terminal_disk_details(identity, slot)? {
            return Ok(details);
        }
        let backend = self.inner.backend()?;
        let (disk, directory, note) = {
            let _notes = crate::lock(&self.inner.disk_notes);
            backend.with_disk_registration(identity, slot, |disk, engine| {
                let note = disk_notes::read(&self.inner.management_root, &disk.disk_id)?;
                let directory = match disk_hardware_root(&engine.config().mode, &disk.root) {
                    None => None,
                    Some(_) => match engine.open_disk_observation_directory(&disk.slot_id) {
                        Ok(directory) => Some(directory),
                        Err(error) => {
                            tracing::debug!(slot_id = %disk.slot_id, error = ?error,
                                "registered disk directory unavailable for hardware observation");
                            None
                        }
                    },
                };
                Ok((disk.clone(), directory, note))
            })?
        };
        let root_path = disk
            .root
            .to_str()
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::InternalError,
                    "registered disk directory cannot be represented as UTF-8",
                )
            })?
            .to_owned();
        let hardware = match directory.as_ref() {
            Some(directory) => directory
                .observe(|access| Ok(crate::disk_hardware::observe(&disk.root, access)))
                .map_err(ApiError::invalid_source)?,
            None => crate::disk_hardware::HardwareObservation::unknown(),
        };
        let health = match directory.as_ref() {
            Some(directory) => {
                crate::disk_hardware::health::observe(&disk.root, directory, &hardware)
            }
            None => storage_contract::disk_details::DiskHealth::Unavailable {
                reason: storage_contract::disk_details::DiskHealthUnavailable::MappingUnknown,
            },
        };
        // SMART runs outside engine/auth locks; revalidate authorization and the
        // current registration before publishing an observation on this card.
        backend.with_disk_registration(identity, slot, |current, engine| {
            if current.disk_id != disk.disk_id || current.root != disk.root {
                return Err(ApiError::invalid(
                    "disk registration changed during observation",
                ));
            }
            if let Some(directory) = directory.as_ref() {
                use std::os::unix::fs::MetadataExt;
                let current = engine
                    .open_disk_observation_directory(&current.slot_id)
                    .map_err(ApiError::invalid_source)?;
                let held = directory.metadata().map_err(ApiError::invalid_source)?;
                let now = current.metadata().map_err(ApiError::invalid_source)?;
                if (held.dev(), held.ino()) != (now.dev(), now.ino()) {
                    return Err(ApiError::invalid(
                        "registered directory changed during observation",
                    ));
                }
            }
            Ok(())
        })?;
        let crate::disk_hardware::HardwareObservation {
            model,
            serial,
            capacity_bytes,
            device_path,
            mount_path,
            filesystem_uuid,
            mapping,
        } = hardware;
        let mapping = match mapping {
            crate::disk_hardware::HardwareMapping::Single => {
                storage_contract::disk_details::DiskMapping::Single
            }
            crate::disk_hardware::HardwareMapping::Multiple => {
                storage_contract::disk_details::DiskMapping::Multiple
            }
            crate::disk_hardware::HardwareMapping::Unknown => {
                storage_contract::disk_details::DiskMapping::Unknown
            }
        };
        Ok(storage_contract::disk_details::DiskDetailsRecord {
            slot_id: disk.slot_id,
            disk_id: disk.disk_id,
            root_path,
            mount_path,
            model,
            serial,
            capacity_bytes,
            device_path,
            filesystem_uuid,
            mapping,
            health,
            note,
        })
    }
    fn update_disk_note(
        &self,
        identity: &Identity,
        slot: &str,
        request: storage_contract::disk_details::DiskNoteRequest,
    ) -> Result<storage_contract::disk_details::DiskNoteResponse, ApiError> {
        if let Some(terminal) = self.inner.terminal(identity)? {
            return self.update_terminal_disk_note(identity, &terminal, slot, request);
        }
        let backend = self.inner.backend()?;
        // Queue before admission: a waiting mutation must see current credentials
        // and the registration that actually owns its persistent note filename.
        let _notes = crate::lock(&self.inner.disk_notes);
        backend.with_disk_registration(identity, slot, |disk, _engine| {
            if request.expected_disk_id != disk.disk_id {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::DiskIdentityMismatch,
                    "registered disk changed; reload before updating its note",
                ));
            }
            let note = disk_notes::update(
                &self.inner.management_root,
                &disk.disk_id,
                &request.expected_note,
                request.note,
            )?;
            Ok(storage_contract::disk_details::DiskNoteResponse {
                slot_id: disk.slot_id.clone(),
                disk_id: disk.disk_id.clone(),
                note,
            })
        })
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
    fn merge_file_history(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MergeFileHistory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.backend()?.merge_file_history(identity, request)
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
    fn placement_configuration(
        &self,
        identity: &Identity,
        request: crate::ReadPlacementConfiguration,
    ) -> Result<storage_contract::PlacementConfigurationSnapshot, ApiError> {
        self.inner
            .backend()?
            .placement_configuration(identity, request)
    }
    fn configure_placement(
        &self,
        identity: &Identity,
        request: crate::ConfigurePlacement,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.inner.backend()?.configure_placement(identity, request)
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
    fn file_history_source(
        &self,
        identity: &Identity,
        request: storage_contract::file_history_merge::FileHistorySourceRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::file_history_merge::FileHistorySource, crate::ApiError> {
        self.inner
            .backend()?
            .file_history_source(identity, request, cancellation)
    }
    fn directory_history(
        &self,
        identity: &crate::auth::Identity,
        request: storage_contract::directory_history::DirectoryHistoryRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::directory_history::DirectoryHistoryPage, crate::ApiError> {
        self.inner
            .backend()?
            .directory_history(identity, request, cancellation)
    }
    fn history_window(
        &self,
        identity: &crate::auth::Identity,
        request: storage_contract::history_window::HistoryWindowRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::history_window::HistoryWindowPage, crate::ApiError> {
        self.inner
            .backend()?
            .history_window(identity, request, cancellation)
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
        if let Some(terminal) = self.inner.terminal(identity)? {
            return Ok(crate::TopologySnapshot {
                generation: terminal.state().topology_generation,
                slots: terminal
                    .registered_disks()
                    .map(|disk| crate::TopologySlot {
                        slot_id: disk.slot_id.to_owned(),
                        disk_id: disk.disk_id.to_owned(),
                        level: disk.level,
                        upstream_slot_id: disk.upstream_slot_id.map(str::to_owned),
                        metadata_member: disk.metadata_member,
                        binding_generation: disk.binding_generation,
                    })
                    .collect(),
            });
        }
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
        if let Some(terminal) = self.inner.terminal(identity)? {
            let state = terminal.state();
            let start = state
                .events
                .partition_point(|event| event.sequence <= after);
            return crate::engine_backend::capture_event_page(
                state.events[start..].iter(),
                state.sequence,
                after,
            );
        }
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
        use crate::setup_recovery::SetupRecovery;
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
        } else if identity.is_none() && !matches!(action, "status" | "initialize") {
            return Err(ApiError::unauthorized());
        }
        let _directory_lifecycle = if matches!(
            action,
            "storage_directories" | "inspect_storage_directory" | "set_discovery_root"
        ) {
            let gate = self.inner.storage_addition.try_lock().map_err(|_| {
                ApiError::conflict(
                    storage_contract::ErrorCode::StorageRetryRunning,
                    "storage lifecycle selection is in progress",
                )
            })?;
            // Cold terminal state is selected from SYSTEM before any directory
            // browsing, including when its management owner has not opened yet.
            if storage_engine::TerminalManagement::requires_management(&self.inner.data_root)
                .map_err(crate::engine_backend::error)?
                && crate::lock(&self.inner.state).backend.terminal().is_none()
            {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "open the terminal management authority before selecting fresh directories",
                ));
            }
            Some(gate)
        } else {
            None
        };
        let protected = {
            let state = crate::lock(&self.inner.state);
            matches!(
                state.backend,
                BackendState::Draining(_)
                    | BackendState::ChangingAuthority
                    | BackendState::Terminal(_)
                    | BackendState::MetadataResume(_)
                    | BackendState::ResumeRecovery
            )
        };
        if protected
            && (matches!(
                action,
                "recovery_status"
                    | "restore_system"
                    | "offline_check"
                    | "recovery_preview"
                    | "recovery_commit"
            ) || (matches!(
                action,
                "storage_directories" | "inspect_storage_directory" | "set_discovery_root"
            ) && crate::lock(&self.inner.state).backend.terminal().is_none()))
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "continue the original storage authority operation; stopped directories cannot be probed by setup",
            ));
        }
        let mut setup = SetupRecovery::new(
            self.inner.data_root.clone(),
            self.inner.management_root.clone(),
            if matches!(
                action,
                "recovery_status"
                    | "restore_system"
                    | "offline_check"
                    | "recovery_preview"
                    | "recovery_commit"
            ) {
                self.inner.setup_disks.clone()
            } else {
                BTreeMap::new()
            },
        )?
        .with_presets(self.inner.setup_configs.clone())
        .with_workspace_limits(self.inner.retained_workspace_limits)?;
        if self.inner.allow_development_setup {
            setup = setup.allow_development_mode();
        }
        if _directory_lifecycle.is_some() {
            let owner_available = {
                let state = crate::lock(&self.inner.state);
                state.backend.active_engine().is_some() || state.backend.terminal().is_some()
            };
            if !owner_available
                && setup.management_status()
                    != crate::setup_recovery::InstallationState::ManagementReady
            {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "open the existing pool authority before browsing storage directories",
                ));
            }
        }
        let permit = bootstrap
            .map(|token| setup.authorize_bootstrap(token))
            .transpose()?;
        if action == "status" {
            let mut state = setup.management_status();
            if state == crate::setup_recovery::InstallationState::ManagementReady
                && !crate::lock(&auth).is_available()
            {
                state = crate::setup_recovery::InstallationState::RecoveryRequired;
            }
            return Ok(json!({"state":state}));
        }
        if action == "recovery_status" {
            return Ok(json_codec::to_value(setup.status())
                .expect("invariant: recovery status serializes"));
        }
        if matches!(
            action,
            "disk_add_preview"
                | "disk_add_commit"
                | "disk_add_status"
                | "disk_add_resume"
                | "disk_add_cancel"
        ) {
            return self.storage_addition(
                action,
                input,
                identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                || {
                    setup
                        .discovery_root()
                        .map(|settings| settings.discovery_root)
                },
            );
        }
        if matches!(
            action,
            "disk_exit_preview" | "disk_exit_commit" | "disk_exit_status" | "disk_exit_resume"
        ) {
            return self.storage_exit(
                action,
                input,
                identity.as_ref().ok_or_else(ApiError::unauthorized)?,
            );
        }
        if matches!(
            action,
            "disk_readmit_preview"
                | "disk_readmit_commit"
                | "disk_readmit_status"
                | "disk_readmit_resume"
                | "disk_registrations"
        ) {
            return self.storage_readmission(
                action,
                input,
                identity.as_ref().ok_or_else(ApiError::unauthorized)?,
            );
        }
        if action == "storage_status" {
            let identity = identity.as_ref().ok_or_else(ApiError::unauthorized)?;
            if let Some(status) = self.inner.terminal_storage_status(identity)? {
                return Ok(json_codec::to_value(status)
                    .expect("invariant: terminal storage status serializes"));
            }
            return Ok(json_codec::to_value(setup.storage_status())
                .expect("invariant: storage activation status serialization succeeds"));
        }
        if action == "discovery_root" {
            identity.as_ref().ok_or_else(ApiError::unauthorized)?;
            return Ok(json_codec::to_value(setup.discovery_root()?)
                .expect("invariant: discovery settings serialize"));
        }
        if action == "set_discovery_root" {
            let request: storage_contract::storage_directories::SetDiscoveryRootRequest =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            if let Some(terminal) = self
                .inner
                .terminal(identity.as_ref().ok_or_else(ApiError::unauthorized)?)?
            {
                return setup
                    .set_terminal_discovery_root(
                        identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                        request,
                        &terminal,
                        |identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ()),
                    )
                    .map(|value| {
                        json_codec::to_value(value)
                            .expect("invariant: discovery settings serialize")
                    });
            }
            let backend = crate::lock(&self.inner.state)
                .backend
                .active_engine()
                .cloned();
            if let Some(backend) = backend {
                let (_, stopped) = backend.addition_directory_scope(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                )?;
                return setup
                    .set_active_discovery_root(
                        identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                        request,
                        &stopped,
                        |identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ()),
                    )
                    .map(|value| {
                        json_codec::to_value(value)
                            .expect("invariant: discovery settings serialize")
                    });
            }
            return setup
                .set_discovery_root(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                    request,
                    |identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ()),
                )
                .map(|value| {
                    json_codec::to_value(value).expect("invariant: discovery settings serialize")
                });
        }
        if action == "storage_directories" {
            identity.as_ref().ok_or_else(ApiError::unauthorized)?;
            #[derive(data_serialization::Deserialize)]
            #[serialization(deny_unknown_fields)]
            struct Request {
                path: String,
            }
            let request: Request =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            if let Some(terminal) = self
                .inner
                .terminal(identity.as_ref().ok_or_else(ApiError::unauthorized)?)?
            {
                let stopped: Vec<_> = terminal
                    .original_configuration()
                    .disks
                    .iter()
                    .map(|disk| disk.root.clone())
                    .collect();
                return crate::storage_directories::browse_fresh_within(
                    &setup.discovery_root()?.discovery_root,
                    &request.path,
                    &stopped,
                )
                .map_err(ApiError::from)
                .map(|value| {
                    json_codec::to_value(value).expect("invariant: directory listing serializes")
                });
            }
            let backend = crate::lock(&self.inner.state)
                .backend
                .active_engine()
                .cloned();
            if let Some(backend) = backend {
                let (_, stopped) = backend.addition_directory_scope(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                )?;
                return crate::storage_directories::browse_fresh_within(
                    &setup.discovery_root()?.discovery_root,
                    &request.path,
                    &stopped,
                )
                .map_err(ApiError::from)
                .map(|value| {
                    json_codec::to_value(value).expect("invariant: directory listing serializes")
                });
            }
            return crate::storage_directories::browse_within(
                &setup.discovery_root()?.discovery_root,
                &request.path,
            )
            .map_err(ApiError::from)
            .map(|value| {
                json_codec::to_value(value).expect("invariant: directory listing serializes")
            });
        }
        if action == "inspect_storage_directory" {
            identity.as_ref().ok_or_else(ApiError::unauthorized)?;
            let request: storage_contract::storage_directories::InspectRequest =
                json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            let mut excluded = vec![
                self.inner.data_root.clone(),
                self.inner.management_root.clone(),
            ];
            if let Some(terminal) = self
                .inner
                .terminal(identity.as_ref().ok_or_else(ApiError::unauthorized)?)?
            {
                let stopped: Vec<_> = terminal
                    .original_configuration()
                    .disks
                    .iter()
                    .map(|disk| disk.root.clone())
                    .collect();
                return crate::storage_directories::inspect_fresh_within(
                    &setup.discovery_root()?.discovery_root,
                    &request,
                    &excluded,
                    &stopped,
                )
                .map_err(ApiError::from)
                .map(|value| {
                    json_codec::to_value(value).expect("invariant: directory selection serializes")
                });
            }
            let backend = crate::lock(&self.inner.state)
                .backend
                .active_engine()
                .cloned();
            if let Some(backend) = backend {
                let (config, stopped) = backend.addition_directory_scope(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                )?;
                excluded.extend(
                    config
                        .disks
                        .into_iter()
                        .filter(|disk| !stopped.contains(&disk.root))
                        .map(|disk| disk.root),
                );
                return crate::storage_directories::inspect_fresh_within(
                    &setup.discovery_root()?.discovery_root,
                    &request,
                    &excluded,
                    &stopped,
                )
                .map_err(ApiError::from)
                .map(|value| {
                    json_codec::to_value(value).expect("invariant: directory selection serializes")
                });
            }
            return crate::storage_directories::inspect_within(
                &setup.discovery_root()?.discovery_root,
                &request,
                &excluded,
            )
            .map_err(ApiError::from)
            .map(|value| {
                json_codec::to_value(value).expect("invariant: directory selection serializes")
            });
        }
        if action == "initialize" {
            let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
            // setup.lock and create-new own the one-time account transition.
            // Anonymous rejected setup must not claim the storage recovery gate
            // or consume a wakeup belonging to an authenticated credential change.
            return setup
                .initialize_management(request, |username, password| {
                    let new_auth = AuthStore::initialize_management(
                        &self.inner.management_root,
                        username,
                        password,
                    )?;
                    claim_authority(&self.inner.data_root)?;
                    crate::lock(&auth).replace_authority(new_auth);
                    // Finish the in-memory transition before releasing setup.lock,
                    // so a later activation cannot be overwritten by late setup.
                    // Pair with worker exit to refresh any pre-account observation.
                    let _worker = crate::lock(&self.inner.recovery_worker);
                    self.inner.set(None, StorageState::Unconfigured);
                    self.inner.generation.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .map(|_| json!({"initialized":true,"storage_pending":true}));
        }
        if matches!(
            crate::lock(&self.inner.state).backend,
            BackendState::Draining(_)
                | BackendState::ChangingAuthority
                | BackendState::Terminal(_)
                | BackendState::MetadataResume(_)
                | BackendState::ResumeRecovery
        ) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "terminal management requires an authenticated metadata resume; initial setup and disk restore cannot reset it",
            ));
        }
        // Keep typed admission before lifecycle changes; later execution owns
        // these same DTOs, not a second decode of the original JSON.
        let command = SetupCommand::parse(action, input)?;
        let _addition = self.inner.storage_addition.try_lock().map_err(|_| {
            ApiError::conflict(
                storage_contract::ErrorCode::StorageRetryRunning,
                "a storage addition is running",
            )
        })?;
        let resumes_storage = command.resumes_storage();
        if self.inner.recovering.swap(true, Ordering::SeqCst) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::StorageRetryRunning,
                "another storage lifecycle operation is running",
            ));
        }
        let lifecycle = RecoveryPermit(&self.inner.recovering);
        {
            let mut state = crate::lock(&self.inner.state);
            if let Some(backend) = state.backend.engine() {
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
                state.backend = BackendState::Unopened;
                state.status = StorageState::RecoveryRequired;
            }
        }
        let revalidate =
            |identity: &Identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ());
        let install_storage = |engine: storage_engine::Engine| {
            let current = identity.as_ref().ok_or_else(ApiError::unauthorized)?;
            let current_auth = crate::lock(&auth);
            current_auth.revalidate_identity(current)?;
            let mut snapshot = current_auth.snapshot()?;
            snapshot["sessions"] = json!({});
            EngineBackend::new(engine).replicate_auth(snapshot)
        };
        let result = match command {
            SetupCommand::Activate(request) => setup
                .activate(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                    request,
                    revalidate,
                    install_storage,
                )
                .map(|_| json!({"activated":true,"restart_required":false,"recovering":true})),
            SetupCommand::CompleteActivation => setup
                .complete_activation(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                    revalidate,
                    install_storage,
                )
                .map(|_| json!({"activated":true,"restart_required":false,"recovering":true})),
            SetupCommand::RestoreSystem(request) => {
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
            SetupCommand::OfflineCheck(disk) => {
                let identity = identity.as_ref().ok_or_else(ApiError::unauthorized)?;
                setup.offline_check(identity, &disk).map(|value| {
                    json_codec::to_value(value)
                        .expect("invariant: internal response serialization succeeds")
                })
            }
            SetupCommand::RecoveryPreview(request) => setup
                .recovery_preview(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                    request,
                    |identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ()),
                )
                .map(|value| {
                    json_codec::to_value(value)
                        .expect("invariant: internal response serialization succeeds")
                }),
            SetupCommand::RecoveryCommit(request) => setup
                .recovery_commit(
                    identity.as_ref().ok_or_else(ApiError::unauthorized)?,
                    request,
                    |identity| crate::lock(&auth).revalidate_identity(identity).map(|_| ()),
                )
                .map(|value| {
                    json_codec::to_value(value)
                        .expect("invariant: internal response serialization succeeds")
                }),
        };
        drop(lifecycle);
        if result.is_ok() && resumes_storage {
            return self.inner.resume_after_setup(result?);
        }
        result
    }

    fn start_management(&self, auth: Arc<Mutex<AuthStore>>) {
        if self.inner.ensure_running().is_err() {
            return;
        }
        let reader = crate::lock(&auth).reader();
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
        if let Err(error) = self.scrub.start(Arc::downgrade(&self.inner), reader) {
            tracing::error!(error = ?error, "integrity scrub scheduler could not start");
        }
        let inner = Arc::clone(&self.inner);
        if let Err(error) = self.observer.start(
            std::thread::Builder::new().name("zxs-disk-observation".into()),
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(5),
            move || {
                if !inner.stopping.load(Ordering::SeqCst) {
                    inner.observe_storage_once();
                }
            },
        ) {
            tracing::warn!(error = ?error, "disk observation worker could not start");
        }
    }
    fn stop_management(&self) {
        self.scrub.stop();
        self.inner.stop();
        self.observer.stop();
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        self.stop_management();
        // The recovery worker takes its own inventory lock at exit, so the
        // public drain obligation is serialized separately from worker locks.
        let _drain = match self.drain.lock() {
            Ok(drain) => drain,
            Err(poisoned) => poisoned.into_inner(),
        };
        // All native owners must drain, even if any reports failure.
        let observer = self.observer.drain();
        let scrub = self.scrub.drain();
        let recovery = self.inner.drain();
        let management = match (scrub, recovery) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(scrub), Err(recovery)) => {
                Err(ApiError::io(std::io::Error::other(ManagementDrainErrors {
                    scrub,
                    recovery,
                })))
            }
        };
        match (management, observer) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(management), Err(observer)) => {
                Err(ApiError::io(std::io::Error::other(ObserverDrainErrors {
                    management,
                    observer,
                })))
            }
        }
    }
    fn start_integrity_scrub(
        &self,
        identity: &Identity,
    ) -> Result<crate::integrity_scrub::IntegrityScrubJobId, ApiError> {
        self.scrub.enqueue(&self.inner, identity)
    }
    fn integrity_scrub_status(
        &self,
        identity: &Identity,
    ) -> Result<crate::integrity_scrub::IntegrityScrubStatus, ApiError> {
        self.scrub.status(&self.inner, identity)
    }
    fn cancel_integrity_scrub(
        &self,
        identity: &Identity,
        job: crate::integrity_scrub::IntegrityScrubJobId,
    ) -> Result<(), ApiError> {
        self.scrub.cancel(identity, job)
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
        let _addition = self.inner.storage_addition.try_lock().map_err(|_| {
            ApiError::conflict(
                storage_contract::ErrorCode::StorageRetryRunning,
                "a storage addition is running",
            )
        })?;
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
#[path = "../../web/src/setup/response.rs"]
mod web_setup_response;

#[cfg(test)]
#[path = "../../web/src/admin/storage/response.rs"]
mod web_activation_response;

#[cfg(test)]
#[path = "../../../test/server/unit/management/disk_cards.rs"]
mod disk_card_tests;

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

#[cfg(test)]
#[path = "../../../test/server/unit/management_admission_tests.rs"]
mod admission_tests;

#[derive(Debug, error_definition::Error)]
#[error("management drain failed: {management}; disk observer drain failed: {observer}")]
struct ObserverDrainErrors {
    #[source]
    management: ApiError,
    observer: ApiError,
}

impl Drop for ManagementBackend {
    fn drop(&mut self) {
        if let Err(error) = self.drain_management() {
            tracing::error!(error = ?error, "management backend shutdown failed");
        }
    }
}
