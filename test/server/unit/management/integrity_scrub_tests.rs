use super::*;
use crate::{
    Backend,
    auth::AuthStore,
    integrity_scrub::{IntegrityScrubFreshness, IntegrityScrubPhase, IntegrityScrubTrigger},
};
use std::{num::NonZeroU64, path::PathBuf, sync::mpsc};
use storage_engine::{Config, Engine, IntegrityScrubCompletion, OperationId, RepositoryName};

const PASSWORD: &str = "integrity-scrub-test-password";
const DEADLINE: Duration = Duration::from_secs(10);

struct Fixture {
    backend: Arc<super::super::ManagementBackend>,
    auth: Arc<Mutex<AuthStore>>,
    identity: Identity,
    token: String,
    data: PathBuf,
    _directory: tempfile::TempDir,
}
impl Fixture {
    fn new(policy: IntegrityScrubPolicy) -> Self {
        let fixture = Self::dormant(policy);
        fixture.start();
        fixture
    }
    fn dormant(policy: IntegrityScrubPolicy) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("system");
        let management = directory.path().join("management");
        let mut engine =
            Engine::initialize(&data, Config::development([directory.path().join("disk")]))
                .unwrap();
        engine
            .create_repository(
                &RepositoryName::parse("scrub-fixture").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        drop(AuthStore::initialize_management(&management, "admin", PASSWORD).unwrap());
        let (backend, mut auth) = super::super::open_service_with_options(
            &data,
            &management,
            super::super::ManagementOptions {
                scrub_policy: policy,
                ..Default::default()
            },
        )
        .unwrap();
        let login = auth.login("admin", PASSWORD).unwrap();
        let identity = auth.authenticate(&login.token).unwrap();
        let reader = auth.reader();
        let auth = Arc::new(Mutex::new(auth));
        *crate::lock(&backend.inner.auth) = Some(Arc::clone(&auth));
        let engine = Arc::new(EngineBackend::new(engine));
        engine.set_management_auth(reader).unwrap();
        backend.inner.set(Some(engine), crate::StorageState::Ready);
        Self {
            backend,
            auth,
            identity,
            token: login.token,
            data,
            _directory: directory,
        }
    }
    fn start(&self) {
        self.backend
            .scrub
            .start(
                Arc::downgrade(&self.backend.inner),
                crate::lock(&self.auth).reader(),
            )
            .unwrap();
    }
    fn engine(&self) -> Arc<EngineBackend> {
        self.backend.inner.backend().unwrap()
    }
    fn status(&self) -> IntegrityScrubStatus {
        self.backend.integrity_scrub_status(&self.identity).unwrap()
    }
    fn wait_latest(&self, job: IntegrityScrubJobId) -> Arc<IntegrityScrubRun> {
        eventually(|| self.status().latest.filter(|run| run.job_id == job))
    }
}
fn positive(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}
fn slow_policy() -> IntegrityScrubPolicy {
    IntegrityScrubPolicy::manual(positive(60), positive(1))
}
fn eventually<T>(mut read: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = read() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "scrub did not reach the observed state"
        );
        // Completion polling only; channels establish every asserted race below.
        thread::sleep(Duration::from_millis(2));
    }
}
fn hold_engine(engine: Arc<EngineBackend>) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
    let (entered, entry) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = thread::spawn(move || {
        engine.test_with_engine_locked(|_| {
            entered.send(()).unwrap();
            released.recv().unwrap();
        })
    });
    entry.recv_timeout(DEADLINE).unwrap();
    (release, worker)
}

#[test]
fn single_job_controls_do_not_wait_for_engine_and_cancel_does_not_detach() {
    let fixture = Fixture::new(slow_policy());
    let engine = fixture.engine();
    assert_eq!(
        Arc::strong_count(&engine),
        2,
        "idle scheduler must not own a backend"
    );
    let (release, holder) = hold_engine(Arc::clone(&engine));
    let id = fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    assert_eq!(
        fixture
            .backend
            .start_integrity_scrub(&fixture.identity)
            .unwrap_err()
            .status,
        http::StatusCode::CONFLICT
    );
    let backend = Arc::clone(&fixture.backend);
    let identity = fixture.identity.clone();
    let (done, received) = mpsc::channel();
    let controls = thread::spawn(move || {
        let status = backend.integrity_scrub_status(&identity).unwrap();
        assert!(matches!(status.phase, IntegrityScrubPhase::Queued { job_id, .. } if job_id == id));
        backend.cancel_integrity_scrub(&identity, id).unwrap();
        done.send(()).unwrap();
    });
    let result = received.recv_timeout(DEADLINE);
    let before_release = fixture
        .backend
        .scrub
        .control
        .state
        .lock()
        .unwrap()
        .latest
        .is_none();
    release.send(()).unwrap();
    holder.join().unwrap();
    controls.join().unwrap();
    result.expect("status and cancel must not acquire Engine");
    assert!(
        before_release,
        "cancel cannot publish completion while worker still waits on Engine"
    );
    let run = fixture.wait_latest(id);
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("cancel must retain its scan report");
    };
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
    assert!(!report.is_healthy());
    eventually(|| (Arc::strong_count(&engine) == 2).then_some(()));
}

#[test]
fn queued_manual_identity_is_revalidated_before_any_scan() {
    let mut fixture = Fixture::new(IntegrityScrubPolicy::default());
    let (release, holder) = hold_engine(fixture.engine());
    let id = fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    {
        let mut auth = crate::lock(&fixture.auth);
        auth.logout(&fixture.token).unwrap();
        let login = auth.login("admin", PASSWORD).unwrap();
        fixture.identity = auth.authenticate(&login.token).unwrap();
    }
    release.send(()).unwrap();
    holder.join().unwrap();
    let run = fixture.wait_latest(id);
    let IntegrityScrubOutcome::Rejected(error) = &run.outcome else {
        panic!("revoked queue admission cannot produce a scan report");
    };
    assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
    assert!(fixture.status().last_complete_success_at.is_none());
}

#[test]
fn paced_real_reads_leave_engine_available_and_cancel_wakes_the_budget_wait() {
    let fixture = Fixture::new(slow_policy());
    let id = fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    eventually(|| match fixture.status().phase {
        IntegrityScrubPhase::Running {
            bytes_read,
            baseline,
            ..
        } if bytes_read > 0 => Some(baseline),
        _ => None,
    });
    let engine = fixture.engine();
    let (done, received) = mpsc::channel();
    let foreground = thread::spawn(move || done.send(engine.read_only()).unwrap());
    let result = received.recv_timeout(DEADLINE);
    fixture
        .backend
        .cancel_integrity_scrub(&fixture.identity, id)
        .unwrap();
    foreground.join().unwrap();
    result
        .expect("real paced I/O must be outside Engine")
        .unwrap();
    let run = fixture.wait_latest(id);
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("cancel retains observed coverage");
    };
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
}

#[test]
fn periodic_worker_really_runs_again_without_manual_dispatch() {
    let fixture = Fixture::new(
        IntegrityScrubPolicy::periodic(positive(1), positive(60), positive(67_108_864)).unwrap(),
    );
    let first = eventually(|| fixture.status().latest);
    assert_eq!(first.trigger, IntegrityScrubTrigger::Periodic);
    let second = eventually(|| {
        fixture
            .status()
            .latest
            .filter(|run| run.job_id != first.job_id)
    });
    assert_eq!(second.trigger, IntegrityScrubTrigger::Periodic);
    let IntegrityScrubOutcome::Report(report) = &second.outcome else {
        panic!("automatic scan must execute core");
    };
    assert!(report.is_healthy());
}

#[test]
fn completed_success_is_not_refreshed_by_writes_and_expires_from_scan_start() {
    let fixture = Fixture::new(IntegrityScrubPolicy::default());
    let id = fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    let run = fixture.wait_latest(id);
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("scan report");
    };
    assert!(report.is_healthy());
    let before = fixture.status();
    assert_eq!(before.freshness, IntegrityScrubFreshness::Fresh);
    fixture
        .engine()
        .create_repository(
            &fixture.identity,
            crate::CreateRepository {
                name: RepositoryName::parse("after-scrub").unwrap(),
                primary_slot_id: "P01".into(),
                target_copies: 1,
                operation_id: OperationId::generate(),
            },
        )
        .unwrap();
    assert_eq!(
        fixture.status().last_complete_success_at,
        before.last_complete_success_at
    );
    let control = &fixture.backend.scrub.control;
    crate::lock(&control.state)
        .success
        .as_mut()
        .unwrap()
        .started = Instant::now() - control.policy.max_age();
    assert_eq!(fixture.status().freshness, IntegrityScrubFreshness::Expired);
    assert_eq!(
        fixture.status().last_complete_success_at,
        before.last_complete_success_at
    );
}

#[test]
fn idle_scheduler_and_old_report_do_not_prevent_engine_reopen() {
    let fixture = Fixture::new(IntegrityScrubPolicy::default());
    let id = fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    fixture.wait_latest(id);
    eventually(|| (Arc::strong_count(&fixture.engine()) == 2).then_some(()));
    fixture
        .backend
        .inner
        .set(None, crate::StorageState::Unavailable);
    let reopened = Arc::new(EngineBackend::new(
        Engine::open_for_recovery(&fixture.data).unwrap(),
    ));
    reopened
        .set_management_auth(crate::lock(&fixture.auth).reader())
        .unwrap();
    fixture
        .backend
        .inner
        .set(Some(reopened), crate::StorageState::Ready);
    assert_eq!(fixture.status().freshness, IntegrityScrubFreshness::Expired);
}

#[test]
fn queued_worker_panic_is_visible_and_drain_returns_the_native_failure() {
    let fixture = Fixture::new(IntegrityScrubPolicy::default());
    let engine = fixture.engine();
    assert!(
        thread::spawn(move || engine.test_with_engine_locked(|_| panic!("poisoned engine fixture")))
            .join()
            .is_err()
    );
    fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    eventually(|| {
        matches!(
            fixture.status().phase,
            IntegrityScrubPhase::SchedulerUnavailable(_)
        )
        .then_some(())
    });
    let error = fixture.backend.drain_management().unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InternalError);
    assert!(format!("{error:?}").contains("integrity scrub worker panicked"));
}

#[test]
fn shutdown_fences_admission_and_joins_a_queued_worker() {
    let fixture = Fixture::new(IntegrityScrubPolicy::default());
    let (release, holder) = hold_engine(fixture.engine());
    fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    // Wait until the worker, rather than the queue, owns the job.
    eventually(|| {
        matches!(
            crate::lock(&fixture.backend.scrub.control.state).activity,
            Activity::Executing(_)
        )
        .then_some(())
    });
    fixture.backend.stop_management();
    assert_eq!(
        fixture
            .backend
            .start_integrity_scrub(&fixture.identity)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ServiceStopping
    );
    let backend = Arc::clone(&fixture.backend);
    let (started, entering) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let drain = thread::spawn(move || {
        started.send(()).unwrap();
        done.send(backend.drain_management()).unwrap();
    });
    entering.recv_timeout(DEADLINE).unwrap();
    assert!(matches!(
        finished.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    release.send(()).unwrap();
    holder.join().unwrap();
    finished.recv_timeout(DEADLINE).unwrap().unwrap();
    drain.join().unwrap();
}

#[test]
fn concurrent_drains_both_wait_for_the_actual_worker() {
    let fixture = Fixture::new(IntegrityScrubPolicy::default());
    let (release, holder) = hold_engine(fixture.engine());
    fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    eventually(|| {
        matches!(
            crate::lock(&fixture.backend.scrub.control.state).activity,
            Activity::Executing(_)
        )
        .then_some(())
    });
    let (enter, entered) = mpsc::channel();
    let (finish, finished) = mpsc::channel();
    let drains = (0..2)
        .map(|_| {
            let backend = Arc::clone(&fixture.backend);
            let enter = enter.clone();
            let finish = finish.clone();
            thread::spawn(move || {
                enter.send(()).unwrap();
                finish.send(backend.drain_management()).unwrap();
            })
        })
        .collect::<Vec<_>>();
    entered.recv_timeout(DEADLINE).unwrap();
    entered.recv_timeout(DEADLINE).unwrap();
    // Both calls have entered; neither may finish while the real Engine
    // admission is blocked. Timeout bounds this negative observation only.
    let early = finished.recv_timeout(Duration::from_millis(100));
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)));
    for drain in drains {
        finished.recv_timeout(DEADLINE).unwrap().unwrap();
        drain.join().unwrap();
    }
}

async fn http_request(
    app: &http_routing::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (http::StatusCode, json_codec::Value) {
    use http_routing::{body::Body, http::Request};
    use service_stack::ServiceExt;
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        json_codec::Value::Null
    } else {
        json_codec::from_slice(&bytes).unwrap()
    };
    (status, value)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_http_controls_expose_baseline_progress_and_keep_check_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("system");
    let management = directory.path().join("management");
    drop(Engine::initialize(&data, Config::development([directory.path().join("disk")])).unwrap());
    drop(AuthStore::initialize_management(&management, "admin", PASSWORD).unwrap());
    let (backend, auth) = super::super::open_service_with_options(
        &data,
        &management,
        super::super::ManagementOptions {
            scrub_policy: slow_policy(),
            ..Default::default()
        },
    )
    .unwrap();
    let app = crate::router(
        backend.clone(),
        auth,
        crate::ServerConfig::loopback(&management),
    )
    .unwrap();
    let deadline = Instant::now() + DEADLINE;
    while backend.inner.backend().is_err() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    for (method, path) in [
        ("GET", "/api/admin/scrub"),
        ("POST", "/api/admin/scrub"),
        ("POST", "/api/admin/scrub/1/cancel"),
    ] {
        assert_eq!(
            http_request(&app, method, path, None, "{}").await.0,
            http::StatusCode::UNAUTHORIZED
        );
    }
    let (status, login) = http_request(
        &app,
        "POST",
        "/api/auth/login",
        None,
        &format!("{{\"username\":\"admin\",\"password\":\"{PASSWORD}\"}}"),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let token = login["token"].as_str().unwrap();
    let (status, initial) = http_request(&app, "GET", "/api/admin/scrub", Some(token), "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(initial["freshness"], "not_checked");
    assert!(initial["last_complete_success_at"].is_null());
    assert_eq!(
        http_request(
            &app,
            "POST",
            "/api/admin/scrub",
            Some(token),
            "{\"bytes_per_second\":1}"
        )
        .await
        .0,
        http::StatusCode::BAD_REQUEST
    );
    let (status, started) = http_request(&app, "POST", "/api/admin/scrub", Some(token), "{}").await;
    assert_eq!(status, http::StatusCode::ACCEPTED);
    let job = started["job_id"].as_str().unwrap();
    assert_eq!(
        http_request(&app, "POST", "/api/admin/scrub", Some(token), "{}")
            .await
            .0,
        http::StatusCode::CONFLICT
    );
    let deadline = Instant::now() + DEADLINE;
    let running = loop {
        let (_, status) = http_request(&app, "GET", "/api/admin/scrub", Some(token), "").await;
        if status["phase"]["state"] == "running"
            && status["phase"]["bytes_read"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                > 0
        {
            break status;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    };
    assert_eq!(running["phase"]["job_id"], job);
    assert!(running["phase"]["baseline"]["sequence"].is_string());
    assert!(running["phase"]["baseline"]["snapshot_hash"].is_string());
    assert!(running["phase"]["expected_payload_bytes"].is_string());
    assert!(
        running["phase"]["started_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    assert_eq!(
        http_request(
            &app,
            "POST",
            "/api/admin/scrub/not-a-number/cancel",
            Some(token),
            "{}"
        )
        .await
        .0,
        http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http_request(
            &app,
            "POST",
            &format!("/api/admin/scrub/{job}/cancel"),
            Some(token),
            "{}"
        )
        .await
        .0,
        http::StatusCode::ACCEPTED
    );
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (_, status) = http_request(&app, "GET", "/api/admin/scrub", Some(token), "").await;
        if status["latest"]["job_id"] == job {
            assert_eq!(status["latest"]["outcome"]["report"]["state"], "cancelled");
            assert!(status["last_complete_success_at"].is_null());
            assert_ne!(status["freshness"], "fresh");
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let (status, check) = http_request(&app, "GET", "/api/admin/check", Some(token), "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(check.get("checked_versions").is_some());
    assert!(check.get("state").is_none());
    assert_eq!(
        http_request(&app, "POST", "/api/auth/logout", Some(token), "{}")
            .await
            .0,
        http::StatusCode::OK
    );
    assert_eq!(
        http_request(&app, "GET", "/api/admin/scrub", Some(token), "")
            .await
            .0,
        http::StatusCode::UNAUTHORIZED
    );
    backend.stop_management();
    let backend = Arc::clone(&backend);
    tokio::task::spawn_blocking(move || backend.drain_management())
        .await
        .unwrap()
        .unwrap();
}

fn periodic_policy(interval: u64) -> IntegrityScrubPolicy {
    IntegrityScrubPolicy::periodic(positive(interval), positive(120), positive(67_108_864)).unwrap()
}
fn coverage(engine: &EngineBackend) -> bool {
    engine.test_with_engine_locked(|engine| engine.has_current_integrity_scrub_coverage())
}
fn await_periodic_success(fixture: &Fixture) {
    let run = eventually(|| fixture.status().latest);
    assert_eq!(run.trigger, IntegrityScrubTrigger::Periodic);
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("real periodic report");
    };
    assert!(report.is_healthy());
}

#[test]
fn manual_only_worker_never_registers_or_mints_scheduled_coverage() {
    let fixture = Fixture::new(IntegrityScrubPolicy::default());
    let job = fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    let run = fixture.wait_latest(job);
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("manual report");
    };
    assert!(report.is_healthy());
    assert!(!coverage(&fixture.engine()));
    assert!(
        crate::lock(&fixture.backend.scrub.control.state)
            .schedule
            .upgrade()
            .is_none()
    );
}

#[test]
fn periodic_owner_is_real_does_not_pin_engine_and_stop_revokes_without_engine_lock() {
    let fixture = Fixture::new(periodic_policy(60));
    await_periodic_success(&fixture);
    let engine = fixture.engine();
    assert!(coverage(&engine));
    eventually(|| (Arc::strong_count(&engine) == 2).then_some(()));
    let (release, holder) = hold_engine(Arc::clone(&engine));
    let backend = Arc::clone(&fixture.backend);
    let (done, received) = mpsc::channel();
    let stop = thread::spawn(move || {
        backend.stop_management();
        done.send(()).unwrap();
    });
    let stopped = received.recv_timeout(DEADLINE);
    release.send(()).unwrap();
    holder.join().unwrap();
    stop.join().unwrap();
    stopped.expect("schedule revocation must not acquire Engine");
    assert!(!coverage(&engine));
    fixture.backend.drain_management().unwrap();
}

#[test]
fn stop_during_first_schedule_registration_revokes_the_uninstalled_owner() {
    let fixture = Fixture::dormant(periodic_policy(60));
    let engine = fixture.engine();
    let (release, holder) = hold_engine(Arc::clone(&engine));
    fixture.start();
    eventually(|| {
        matches!(
            crate::lock(&fixture.backend.scrub.control.state).activity,
            Activity::Executing(_)
        )
        .then_some(())
    });
    fixture.backend.stop_management();
    let backend = Arc::clone(&fixture.backend);
    let (done, received) = mpsc::channel();
    let drain = thread::spawn(move || done.send(backend.drain_management()).unwrap());
    let early = received.recv_timeout(Duration::from_millis(100));
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)));
    received.recv_timeout(DEADLINE).unwrap().unwrap();
    drain.join().unwrap();
    assert!(!coverage(&engine));
    // Registration that raced stop must not leave an active Engine owner.
    let replacement = engine
        .begin_integrity_scrub_schedule(periodic_policy(60))
        .unwrap();
    assert!(!coverage(&engine));
    drop(replacement);
}

#[test]
fn native_worker_unwind_drops_its_schedule_qualification() {
    let fixture = Fixture::new(periodic_policy(60));
    await_periodic_success(&fixture);
    let engine = fixture.engine();
    assert!(coverage(&engine));
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = crate::lock(&fixture.backend.scrub.control.state);
            panic!("poison idle scheduler state fixture");
        }))
        .is_err()
    );
    fixture.backend.scrub.control.wake.notify_one();
    eventually(|| {
        crate::lock(&fixture.backend.scrub.worker)
            .as_ref()
            .unwrap()
            .is_finished()
            .then_some(())
    });
    assert!(
        !coverage(&engine),
        "panic must drop the actual lease, not leave a heartbeat proof"
    );
    assert_eq!(
        fixture.backend.drain_management().unwrap_err().code,
        storage_contract::ErrorCode::InternalError
    );
}

#[test]
fn reopened_engine_needs_a_new_lease_and_a_real_scan_even_when_manually_triggered() {
    let fixture = Fixture::new(periodic_policy(60));
    await_periodic_success(&fixture);
    let old_schedule = crate::lock(&fixture.backend.scrub.control.state)
        .schedule
        .clone();
    eventually(|| (Arc::strong_count(&fixture.engine()) == 2).then_some(()));
    fixture
        .backend
        .inner
        .set(None, crate::StorageState::Unavailable);
    let engine = Arc::new(EngineBackend::new(
        Engine::open_for_recovery(&fixture.data).unwrap(),
    ));
    engine
        .set_management_auth(crate::lock(&fixture.auth).reader())
        .unwrap();
    assert!(!coverage(&engine));
    fixture
        .backend
        .inner
        .set(Some(Arc::clone(&engine)), crate::StorageState::Ready);
    let job = fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    let run = fixture.wait_latest(job);
    assert_eq!(run.trigger, IntegrityScrubTrigger::Manual);
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("real manually triggered scan");
    };
    assert!(report.is_healthy());
    assert!(
        coverage(&engine),
        "the continuous periodic owner, not the trigger label, establishes coverage"
    );
    assert!(
        old_schedule.upgrade().is_none(),
        "the old writer lease must be released"
    );
}

#[test]
fn missed_periodic_deadline_expires_while_worker_waits_for_real_engine_admission() {
    let fixture = Fixture::new(periodic_policy(1));
    await_periodic_success(&fixture);
    let engine = fixture.engine();
    assert!(coverage(&engine));
    let held = Arc::clone(&engine);
    let (expired, received) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let holder = thread::spawn(move || {
        held.test_with_engine_locked(|engine| {
            // Real elapsed time passes while the next periodic worker cannot admit.
            thread::sleep(Duration::from_millis(1_100));
            expired
                .send(engine.has_current_integrity_scrub_coverage())
                .unwrap();
            released.recv_timeout(DEADLINE).unwrap();
        })
    });
    assert!(!received.recv_timeout(DEADLINE).unwrap());
    fixture.backend.stop_management();
    release.send(()).unwrap();
    holder.join().unwrap();
    fixture.backend.drain_management().unwrap();
    assert!(!coverage(&engine));
}

#[test]
fn local_proof_failure_keeps_shared_graph_coverage_distinct_from_full_health() {
    let fixture = Fixture::dormant(periodic_policy(60));
    let engine = fixture.engine();
    let proof = engine.test_with_engine_locked(|engine| {
        let repository = engine.state().repositories.values().next().unwrap().clone();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "file.txt",
                b"independent local proof fault",
            )
            .unwrap();
        engine
            .commit_workspace(storage_engine::CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "lease fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.state().versions.values().next().unwrap();
        fixture
            ._directory
            .path()
            .join("disk")
            .join("histories")
            .join(&version.history_tree_id)
            .join("versions")
            .join(&version.version_id)
            .join("publication-proof.json")
    });
    std::fs::remove_file(proof).unwrap();
    fixture.start();
    let run = eventually(|| fixture.status().latest);
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("real partial report");
    };
    assert!(!report.is_healthy());
    assert!(report.metadata_failures > 0);
    assert!(
        coverage(&engine),
        "version-local proof loss is not a shared graph fault"
    );
    engine
        .create_repository(
            &fixture.identity,
            crate::CreateRepository {
                name: RepositoryName::parse("unrelated-after-proof-fault").unwrap(),
                primary_slot_id: "P01".into(),
                target_copies: 1,
                operation_id: OperationId::generate(),
            },
        )
        .expect("local proof health failure must not fence unrelated writes");
    assert_eq!(
        fixture.status().freshness,
        IntegrityScrubFreshness::Unavailable
    );
    assert!(fixture.status().last_complete_success_at.is_none());
}
