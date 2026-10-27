//! One concrete scheduler owns its native worker; idle periods own no backend.
use super::Inner;
use crate::{
    ApiError,
    auth::{AuthReader, Identity},
    engine_backend::EngineBackend,
    integrity_scrub::{
        IntegrityScrubBaseline, IntegrityScrubFreshness, IntegrityScrubJobId,
        IntegrityScrubOutcome, IntegrityScrubPhase, IntegrityScrubPolicy, IntegrityScrubRun,
        IntegrityScrubStatus, ScrubAdmission, ScrubExecution,
    },
};
use std::{
    sync::{Arc, Condvar, Mutex, OnceLock, Weak},
    thread,
    time::{Duration, Instant, SystemTime},
};
use storage_engine::{IntegrityScrubCancellation, IntegrityScrubReport, IntegrityScrubSchedule};

type ScrubWorker = thread::JoinHandle<Result<(), Arc<ApiError>>>;

pub(super) struct IntegrityScrubService {
    control: Arc<Control>,
    worker: Mutex<Option<ScrubWorker>>,
}
struct Control {
    state: Mutex<State>,
    wake: Condvar,
    auth: OnceLock<AuthReader>,
    policy: IntegrityScrubPolicy,
}
struct State {
    lifecycle: Lifecycle,
    activity: Activity,
    next_id: u64,
    latest: Option<Arc<IntegrityScrubRun>>,
    success: Option<Success>,
    last_attempt: Option<Instant>,
    awaiting_storage: bool,
    known_failure: bool,
    schedule: Weak<IntegrityScrubSchedule>,
}
enum Lifecycle {
    Dormant,
    Ready,
    Stopping,
    Failed(Arc<ApiError>),
}
enum Activity {
    Idle,
    Queued(Job),
    Executing(Active),
}
struct Job {
    id: IntegrityScrubJobId,
    backend: Arc<EngineBackend>,
    admission: ScrubAdmission,
    cancellation: Arc<IntegrityScrubCancellation>,
}
struct Active {
    id: IntegrityScrubJobId,
    trigger: crate::integrity_scrub::IntegrityScrubTrigger,
    cancellation: Arc<IntegrityScrubCancellation>,
    progress: Progress,
}
enum Progress {
    Queued,
    Running {
        baseline: IntegrityScrubBaseline,
        started_at: SystemTime,
        started: Instant,
        bytes_read: u128,
        expected_payload_bytes: u64,
    },
}
// Only the native worker owns this lease. The control plane can revoke through
// a Weak, and idle scheduling never retains the Engine or its writer locks.
struct OwnedSchedule {
    backend: Weak<EngineBackend>,
    lease: Arc<IntegrityScrubSchedule>,
}
struct Success {
    finished_at: SystemTime,
    started: Instant,
    backend: Weak<EngineBackend>,
}

impl IntegrityScrubService {
    pub(super) fn new(policy: IntegrityScrubPolicy) -> Self {
        Self {
            control: Arc::new(Control {
                state: Mutex::new(State {
                    lifecycle: Lifecycle::Dormant,
                    activity: Activity::Idle,
                    next_id: 1,
                    latest: None,
                    success: None,
                    last_attempt: None,
                    awaiting_storage: false,
                    known_failure: false,
                    schedule: Weak::new(),
                }),
                wake: Condvar::new(),
                auth: OnceLock::new(),
                policy,
            }),
            worker: Mutex::new(None),
        }
    }
    pub(super) fn start(&self, inner: Weak<Inner>, auth: AuthReader) -> Result<(), Arc<ApiError>> {
        let mut worker = crate::lock(&self.worker);
        let mut state = crate::lock(&self.control.state);
        match &state.lifecycle {
            Lifecycle::Dormant => (),
            Lifecycle::Ready => return Ok(()),
            Lifecycle::Stopping => return Err(Arc::new(stopping())),
            Lifecycle::Failed(error) => return Err(Arc::clone(error)),
        }
        assert!(
            self.control.auth.set(auth).is_ok(),
            "invariant: scrub authority is installed once"
        );
        let control = Arc::clone(&self.control);
        match thread::Builder::new()
            .name("zxs-integrity-scrub".into())
            .spawn(move || worker_entry(control, inner))
        {
            Ok(handle) => {
                *worker = Some(handle);
                state.lifecycle = Lifecycle::Ready;
                Ok(())
            }
            Err(error) => {
                let error = Arc::new(
                    ApiError::new(
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        storage_contract::ErrorCode::InternalError,
                        "integrity scrub worker could not start",
                    )
                    .with_source(error),
                );
                state.lifecycle = Lifecycle::Failed(Arc::clone(&error));
                Err(error)
            }
        }
    }
    fn authorize(&self, identity: &Identity) -> Result<Identity, ApiError> {
        self.control
            .auth
            .get()
            .ok_or_else(ApiError::unauthorized)?
            .revalidate_identity(identity)
    }
    pub(super) fn enqueue(
        &self,
        inner: &Inner,
        identity: &Identity,
    ) -> Result<IntegrityScrubJobId, ApiError> {
        let identity = self.authorize(identity)?;
        let backend = inner.backend()?;
        let mut state = crate::lock(&self.control.state);
        require_ready(&state.lifecycle)?;
        let id = enqueue(&mut state, backend, ScrubAdmission::Manual(identity))?;
        self.control.wake.notify_one();
        Ok(id)
    }
    pub(super) fn cancel(
        &self,
        identity: &Identity,
        id: IntegrityScrubJobId,
    ) -> Result<(), ApiError> {
        self.authorize(identity)?;
        let state = crate::lock(&self.control.state);
        let cancellation = match &state.activity {
            Activity::Queued(job) if job.id == id => &job.cancellation,
            Activity::Executing(job) if job.id == id => &job.cancellation,
            Activity::Idle | Activity::Queued(_) | Activity::Executing(_) => {
                return Err(ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "scrub job is not active",
                ));
            }
        };
        cancellation.cancel();
        self.control.wake.notify_one();
        Ok(())
    }
    pub(super) fn status(
        &self,
        inner: &Inner,
        identity: &Identity,
    ) -> Result<IntegrityScrubStatus, ApiError> {
        self.authorize(identity)?;
        // Ready admission is a short state read, never an Engine operation.
        let backend = inner.backend();
        let state = crate::lock(&self.control.state);
        let phase = match &state.lifecycle {
            Lifecycle::Dormant => IntegrityScrubPhase::NotReady,
            Lifecycle::Stopping => IntegrityScrubPhase::Stopping,
            Lifecycle::Failed(error) => {
                IntegrityScrubPhase::SchedulerUnavailable(Arc::clone(error))
            }
            Lifecycle::Ready => match &state.activity {
                Activity::Idle if backend.is_err() => IntegrityScrubPhase::NotReady,
                Activity::Idle => IntegrityScrubPhase::Idle,
                Activity::Queued(job) => IntegrityScrubPhase::Queued {
                    job_id: job.id,
                    cancellation_requested: job.cancellation.is_cancelled(),
                },
                Activity::Executing(job) => match job.progress {
                    Progress::Queued => IntegrityScrubPhase::Queued {
                        job_id: job.id,
                        cancellation_requested: job.cancellation.is_cancelled(),
                    },
                    Progress::Running {
                        ref baseline,
                        started_at,
                        bytes_read,
                        expected_payload_bytes,
                        ..
                    } => IntegrityScrubPhase::Running {
                        job_id: job.id,
                        baseline: baseline.clone(),
                        started_at,
                        bytes_read,
                        expected_payload_bytes,
                        cancellation_requested: job.cancellation.is_cancelled(),
                    },
                },
            },
        };
        let freshness = match (&state.lifecycle, &state.success, backend) {
            _ if state.known_failure => IntegrityScrubFreshness::Unavailable,
            (Lifecycle::Dormant | Lifecycle::Stopping | Lifecycle::Failed(_), _, _)
            | (_, _, Err(_)) => IntegrityScrubFreshness::Unavailable,
            (_, None, Ok(_)) => IntegrityScrubFreshness::NotChecked,
            (_, Some(success), Ok(backend))
                if success.started.elapsed() < self.control.policy.max_age()
                    && Weak::ptr_eq(&success.backend, &Arc::downgrade(&backend)) =>
            {
                IntegrityScrubFreshness::Fresh
            }
            (_, Some(_), Ok(_)) => IntegrityScrubFreshness::Expired,
        };
        Ok(IntegrityScrubStatus {
            phase,
            latest: state.latest.as_ref().map(Arc::clone),
            last_complete_success_at: state.success.as_ref().map(|success| success.finished_at),
            freshness,
        })
    }
    pub(super) fn stop(&self) {
        // Poison recovery belongs only to shutdown, never renewed admission.
        let mut state = match self.control.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !matches!(state.lifecycle, Lifecycle::Failed(_)) {
            state.lifecycle = Lifecycle::Stopping;
        }
        match &state.activity {
            Activity::Idle => (),
            Activity::Queued(job) => job.cancellation.cancel(),
            Activity::Executing(job) => job.cancellation.cancel(),
        }
        let schedule = state.schedule.upgrade();
        drop(state);
        if let Some(schedule) = schedule {
            schedule.revoke();
        }
        self.control.wake.notify_one();
    }
    pub(super) fn drain(&self) -> Result<(), ApiError> {
        self.stop();
        // Serialize concurrent drains through the actual native join. A second
        // caller must not observe an empty handle while the first still owns I/O.
        let mut worker = match self.worker.lock() {
            Ok(worker) => worker,
            Err(poisoned) => poisoned.into_inner(),
        };
        match worker.take() {
            None => Ok(()),
            Some(worker) => match worker.join() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(shared_error(error)),
                Err(panic) => Err(worker_panic(panic)),
            },
        }
    }
}
impl Drop for IntegrityScrubService {
    fn drop(&mut self) {
        if let Err(error) = self.drain() {
            tracing::error!(error = ?error, "integrity scrub shutdown failed");
        }
    }
}

fn enqueue(
    state: &mut State,
    backend: Arc<EngineBackend>,
    admission: ScrubAdmission,
) -> Result<IntegrityScrubJobId, ApiError> {
    if !matches!(state.activity, Activity::Idle) {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::ResourceLimit,
            "an integrity scrub is already queued or running",
        ));
    }
    let id = IntegrityScrubJobId(state.next_id);
    state.next_id = state
        .next_id
        .checked_add(1)
        .ok_or_else(|| ApiError::limit("integrity scrub job identifiers exhausted"))?;
    state.activity = Activity::Queued(Job {
        id,
        backend,
        admission,
        cancellation: Arc::new(IntegrityScrubCancellation::new()),
    });
    Ok(id)
}
fn require_ready(lifecycle: &Lifecycle) -> Result<(), ApiError> {
    match lifecycle {
        Lifecycle::Ready => Ok(()),
        Lifecycle::Dormant => Err(ApiError::new(
            http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::StorageUnavailable,
            "integrity scrub scheduler has not started",
        )),
        Lifecycle::Stopping => Err(stopping()),
        Lifecycle::Failed(error) => Err(shared_error(Arc::clone(error))),
    }
}
fn stopping() -> ApiError {
    ApiError::new(
        http::StatusCode::SERVICE_UNAVAILABLE,
        storage_contract::ErrorCode::ServiceStopping,
        "integrity scrub service is stopping",
    )
}
fn shared_error(error: Arc<ApiError>) -> ApiError {
    let mut response = ApiError::new(error.status, error.code, error.message.clone());
    response.retryable = error.retryable;
    response.with_source(error)
}

fn worker_entry(control: Arc<Control>, inner: Weak<Inner>) -> Result<(), Arc<ApiError>> {
    // One task boundary, not a catch around each core call or filesystem action.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&control, &inner))) {
        Ok(()) => Ok(()),
        Err(panic) => {
            let error = Arc::new(worker_panic(panic));
            let mut state = crate::lock(&control.state);
            if let Activity::Executing(active) = &state.activity {
                state.latest = Some(Arc::new(IntegrityScrubRun {
                    job_id: active.id,
                    trigger: active.trigger,
                    outcome: IntegrityScrubOutcome::Rejected(shared_error(Arc::clone(&error))),
                }));
            }
            state.activity = Activity::Idle;
            state.lifecycle = Lifecycle::Failed(Arc::clone(&error));
            tracing::error!(error = ?error, "integrity scrub worker failed");
            Err(error)
        }
    }
}
fn run(control: &Arc<Control>, inner: &Weak<Inner>) {
    let mut schedule = None;
    while let Some(job) = next_job(control, inner) {
        let outcome = run_job(control, &job, &mut schedule);
        finish_job(control, job, outcome);
    }
}
fn execution<'a>(
    control: &Control,
    job: &'a Job,
    schedule: &'a mut Option<OwnedSchedule>,
) -> Result<ScrubExecution<'a>, ApiError> {
    if schedule
        .as_ref()
        .is_some_and(|owner| !Weak::ptr_eq(&owner.backend, &Arc::downgrade(&job.backend)))
    {
        // Reopen is a new writer lifetime, never a continuation of old evidence.
        *schedule = None;
    }
    if control.policy.interval().is_none() {
        return match &job.admission {
            ScrubAdmission::Manual(identity) => Ok(ScrubExecution::Standalone(identity)),
            ScrubAdmission::Periodic => {
                unreachable!("invariant: manual-only policy admits no periodic jobs")
            }
        };
    }
    if schedule.is_none() {
        let lease = Arc::new(job.backend.begin_integrity_scrub_schedule(control.policy)?);
        let mut state = crate::lock(&control.state);
        if !matches!(state.lifecycle, Lifecycle::Ready) {
            // stop may have raced the short Engine registration above.
            drop(state);
            lease.revoke();
            return Err(stopping());
        }
        state.schedule = Arc::downgrade(&lease);
        *schedule = Some(OwnedSchedule {
            backend: Arc::downgrade(&job.backend),
            lease,
        });
    }
    Ok(ScrubExecution::Scheduled {
        identity: match &job.admission {
            ScrubAdmission::Manual(identity) => Some(identity),
            ScrubAdmission::Periodic => None,
        },
        owner: &schedule
            .as_ref()
            .expect("invariant: scheduled worker installed its live owner")
            .lease,
    })
}
fn run_job(
    control: &Control,
    job: &Job,
    schedule: &mut Option<OwnedSchedule>,
) -> Result<IntegrityScrubReport, ApiError> {
    let admission = execution(control, job, schedule)?;
    let mut last_read = Instant::now();
    job.backend.run_integrity_scrub(
        admission,
        &job.cancellation,
        |report: &IntegrityScrubReport, expected_payload_bytes| {
            let mut state = crate::lock(&control.state);
            let Activity::Executing(active) = &mut state.activity else {
                unreachable!("invariant: one worker owns the active scrub");
            };
            if matches!(active.progress, Progress::Queued) {
                active.progress = Progress::Running {
                    baseline: IntegrityScrubBaseline {
                        sequence: report.sequence,
                        snapshot_hash: report.snapshot_hash.clone(),
                    },
                    started_at: report.started_at,
                    started: Instant::now(),
                    bytes_read: 0,
                    expected_payload_bytes,
                };
            }
            if report.identity_failures != 0
                || report.metadata_failures != 0
                || report.payload_copies_unavailable != 0
                || report.payload_copies_unverified != 0
                || matches!(
                    report.completion,
                    storage_engine::IntegrityScrubCompletion::Cancelled
                        | storage_engine::IntegrityScrubCompletion::Superseded
                        | storage_engine::IntegrityScrubCompletion::Failed(_)
                )
            {
                state.known_failure = true;
            }
        },
        |bytes| pace(control, &job.cancellation, &mut last_read, bytes),
    )
}

fn next_job(control: &Control, inner: &Weak<Inner>) -> Option<Job> {
    let mut state = crate::lock(&control.state);
    loop {
        if !matches!(state.lifecycle, Lifecycle::Ready) {
            state.activity = Activity::Idle;
            return None;
        }
        if matches!(state.activity, Activity::Queued(_)) {
            let Activity::Queued(job) = std::mem::replace(&mut state.activity, Activity::Idle)
            else {
                unreachable!("invariant: queued job was checked under this lock");
            };
            state.activity = Activity::Executing(Active {
                id: job.id,
                trigger: job.admission.trigger(),
                cancellation: Arc::clone(&job.cancellation),
                progress: Progress::Queued,
            });
            return Some(job);
        }
        let wait = control.policy.interval().map(|interval| {
            let interval = if state.awaiting_storage {
                interval.min(Duration::from_secs(5))
            } else {
                interval
            };
            state.last_attempt.map_or(Duration::ZERO, |last| {
                interval.saturating_sub(last.elapsed())
            })
        });
        if wait == Some(Duration::ZERO) {
            drop(state);
            let owner = inner
                .upgrade()
                .expect("invariant: service owns Inner until scrub drain completes");
            let backend = owner.backend();
            drop(owner);
            state = crate::lock(&control.state);
            state.last_attempt = Some(Instant::now());
            if !matches!(state.lifecycle, Lifecycle::Ready) {
                continue;
            }
            match backend {
                Ok(backend) => {
                    state.awaiting_storage = false;
                    if matches!(state.activity, Activity::Idle)
                        && let Err(error) = enqueue(&mut state, backend, ScrubAdmission::Periodic)
                    {
                        state.lifecycle = Lifecycle::Failed(Arc::new(error));
                    }
                }
                Err(error)
                    if matches!(
                        error.code,
                        storage_contract::ErrorCode::StorageUnavailable
                            | storage_contract::ErrorCode::ServiceStopping
                    ) =>
                {
                    // The storage control plane remains the authority for this
                    // expected NotReady state; no scan has been admitted.
                    state.awaiting_storage = true;
                }
                Err(error) => state.lifecycle = Lifecycle::Failed(Arc::new(error)),
            }
            continue;
        }
        state = match wait {
            Some(wait) => {
                control
                    .wake
                    .wait_timeout(state, wait)
                    .expect("invariant: scrub control lock is not poisoned")
                    .0
            }
            None => control
                .wake
                .wait(state)
                .expect("invariant: scrub control lock is not poisoned"),
        };
    }
}
fn pace(
    control: &Control,
    cancellation: &IntegrityScrubCancellation,
    last_read: &mut Instant,
    bytes: usize,
) {
    let cost = control.policy.read_duration(bytes);
    let mut state = crate::lock(&control.state);
    let Activity::Executing(active) = &mut state.activity else {
        unreachable!("invariant: I/O belongs to the active scrub");
    };
    let Progress::Running { bytes_read, .. } = &mut active.progress else {
        unreachable!("invariant: byte accounting follows baseline admission");
    };
    // Payload estimates are bounded to u64; metadata per-member frame/object
    // budgets times an addressable member list fit this wider progress counter.
    *bytes_read += bytes as u128;
    loop {
        if cancellation.is_cancelled() {
            return;
        }
        let wait = cost.saturating_sub(last_read.elapsed());
        if wait.is_zero() {
            *last_read = Instant::now();
            return;
        }
        state = control
            .wake
            .wait_timeout(state, wait)
            .expect("invariant: scrub control lock is not poisoned")
            .0;
    }
}
fn finish_job(control: &Control, job: Job, outcome: Result<IntegrityScrubReport, ApiError>) {
    let mut state = crate::lock(&control.state);
    let outcome = match outcome {
        Ok(report) => {
            if !report.is_healthy()
                && !matches!(
                    report.completion,
                    storage_engine::IntegrityScrubCompletion::Cancelled
                        | storage_engine::IntegrityScrubCompletion::Superseded
                )
            {
                tracing::warn!(job_id = job.id.0, report = ?report, "integrity scrub found incomplete or damaged storage");
            }
            if report.is_healthy() {
                let Activity::Executing(Active {
                    progress: Progress::Running { started, .. },
                    ..
                }) = &state.activity
                else {
                    unreachable!("invariant: a healthy report completed an admitted scan");
                };
                state.success = Some(Success {
                    finished_at: report.finished_at,
                    started: *started,
                    backend: Arc::downgrade(&job.backend),
                });
                state.known_failure = false;
            }
            if !report.is_healthy() {
                state.known_failure = true;
            }
            IntegrityScrubOutcome::Report(Box::new(report))
        }
        Err(error) => {
            tracing::debug!(job_id = job.id.0, error = ?error, "integrity scrub admission rejected");
            IntegrityScrubOutcome::Rejected(error)
        }
    };
    state.latest = Some(Arc::new(IntegrityScrubRun {
        job_id: job.id,
        trigger: job.admission.trigger(),
        outcome,
    }));
    state.activity = Activity::Idle;
    state.last_attempt = Some(Instant::now());
    state.awaiting_storage = false;
}
#[derive(Debug, error_definition::Error)]
#[error("integrity scrub worker panicked: {message:?}")]
struct WorkerPanic {
    message: Option<String>,
}
fn worker_panic(panic: Box<dyn std::any::Any + Send>) -> ApiError {
    let message = panic.downcast_ref::<String>().cloned().or_else(|| {
        panic
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
    });
    ApiError::new(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::InternalError,
        "integrity scrub worker panicked",
    )
    .with_source(std::io::Error::other(WorkerPanic { message }))
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/integrity_scrub_tests.rs"]
mod tests;
