use super::*;
use std::{sync::mpsc, time::Duration};

const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn completed_setup_preserves_success_when_shutdown_fences_reenqueue() {
    let temporary = tempfile::tempdir().unwrap();
    let (backend, _) = open_service(
        &temporary.path().join("system"),
        &temporary.path().join("management"),
    )
    .unwrap();
    backend.stop_management();
    let completed = backend
        .inner
        .resume_after_setup(json!({"initialized":true,"recovering":true,"restart_required":false}))
        .unwrap();
    assert_eq!(completed["initialized"], true);
    assert_eq!(completed["recovering"], false);
    assert_eq!(completed["restart_required"], true);
    assert!(backend.inner.recovery_worker.lock().unwrap().is_none());
}

#[test]
fn completed_setup_preserves_success_when_retry_already_won_enqueue() {
    let temporary = tempfile::tempdir().unwrap();
    let (backend, _) = open_service(
        &temporary.path().join("system"),
        &temporary.path().join("management"),
    )
    .unwrap();
    backend.inner.recovering.store(true, Ordering::SeqCst);
    let completed = backend
        .inner
        .resume_after_setup(json!({"initialized":true,"recovering":true,"restart_required":false}))
        .unwrap();
    assert_eq!(completed["initialized"], true);
    assert_eq!(completed["recovering"], true);
    assert_eq!(completed["restart_required"], false);
    backend.inner.recovering.store(false, Ordering::SeqCst);
}

#[test]
fn stop_drains_owned_recovery_and_fences_generation_reenqueue() {
    let temporary = tempfile::tempdir().unwrap();
    let (backend, _) = open_service(
        &temporary.path().join("system"),
        &temporary.path().join("management"),
    )
    .unwrap();
    let (entered, entry) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let calls = Arc::new(AtomicU64::new(0));
    let worker_calls = calls.clone();
    let inner = backend.inner.clone();
    let mut worker = backend.inner.recovery_worker.lock().unwrap();
    backend.inner.recovering.store(true, Ordering::SeqCst);
    *worker = Some(std::thread::spawn(move || {
        inner.run_recovery(move |inner| {
            worker_calls.fetch_add(1, Ordering::SeqCst);
            inner.generation.fetch_add(1, Ordering::SeqCst);
            entered.send(()).unwrap();
            released.recv_timeout(DEADLINE).unwrap();
            Ok(())
        })
    }));
    drop(worker);
    entry.recv_timeout(DEADLINE).unwrap();
    backend.stop_management();
    assert_eq!(
        backend.inner.recover().unwrap_err().code,
        storage_contract::ErrorCode::ServiceStopping
    );
    let (draining, started) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let draining_backend = backend.clone();
    let drain = std::thread::spawn(move || {
        draining.send(()).unwrap();
        done.send(draining_backend.drain_management()).unwrap();
    });
    started.recv_timeout(DEADLINE).unwrap();
    assert!(matches!(
        finished.recv_timeout(Duration::from_millis(25)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    finished.recv_timeout(DEADLINE).unwrap().unwrap();
    drain.join().unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "shutdown must not enqueue a generation follow-up"
    );
    assert!(!backend.inner.recovering.load(Ordering::SeqCst));
    assert!(backend.inner.recovery_worker.lock().unwrap().is_none());
    backend.inner.set(None, StorageState::Ready);
    assert!(!backend.inner.state.lock().unwrap().admitted);
    assert_eq!(backend.management_status()["state"], "unavailable");
}

#[test]
fn native_recovery_handle_is_retained_until_explicit_drain() {
    let temporary = tempfile::tempdir().unwrap();
    let (backend, _) = open_service(
        &temporary.path().join("missing"),
        &temporary.path().join("management"),
    )
    .unwrap();
    backend.inner.recover().unwrap();
    assert!(backend.inner.recovery_worker.lock().unwrap().is_some());
    backend.drain_management().unwrap();
    assert!(backend.inner.recovery_worker.lock().unwrap().is_none());
    assert_eq!(
        backend.inner.start_recovery(true).unwrap_err().code,
        storage_contract::ErrorCode::ServiceStopping
    );
    backend.drain_management().unwrap();
}

#[test]
fn auth_changes_coalesce_on_the_same_owned_native_thread() {
    let temporary = tempfile::tempdir().unwrap();
    let (backend, _) = open_service(
        &temporary.path().join("system"),
        &temporary.path().join("management"),
    )
    .unwrap();
    let (entered, entry) = mpsc::channel();
    let inner = backend.inner.clone();
    let mut worker = backend.inner.recovery_worker.lock().unwrap();
    backend.inner.recovering.store(true, Ordering::SeqCst);
    *worker = Some(std::thread::spawn(move || {
        let mut calls = 0;
        inner.run_recovery(move |inner| {
            calls += 1;
            if calls == 1 {
                inner.generation.fetch_add(1, Ordering::SeqCst);
            }
            entered.send(std::thread::current().id()).unwrap();
            Ok(())
        });
    }));
    drop(worker);
    assert_eq!(
        entry.recv_timeout(DEADLINE).unwrap(),
        entry.recv_timeout(DEADLINE).unwrap()
    );
    backend.drain_management().unwrap();
    assert!(!backend.inner.recovering.load(Ordering::SeqCst));
}

fn shutdown_error(error: &ApiError) -> &RecoveryShutdownError {
    let crate::http_boundary::ApiErrorSource::Io(error) = error.source.as_deref().unwrap() else {
        panic!("shutdown failure must keep its concrete IO source");
    };
    error
        .get_ref()
        .unwrap()
        .downcast_ref::<RecoveryShutdownError>()
        .unwrap()
}

#[test]
fn poisoned_exit_state_is_disabled_and_owned_worker_is_joined_during_unwind() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    for (poison_worker, poison_state) in [(true, false), (false, true), (true, true)] {
        let temporary = tempfile::tempdir().unwrap();
        let (backend, _) = open_service(
            &temporary.path().join("system"),
            &temporary.path().join("management"),
        )
        .unwrap();
        let retained = Arc::new(());
        let worker_retained = Arc::clone(&retained);
        let (release, work) = mpsc::channel();
        let (report, outcome) = mpsc::channel();
        let (running, started) = mpsc::channel();
        *backend.inner.recovery_worker.lock().unwrap() = Some(std::thread::spawn(move || {
            let _retained = worker_retained;
            running.send(()).unwrap();
            report.send(work.recv_timeout(DEADLINE)).unwrap();
        }));
        started.recv_timeout(DEADLINE).unwrap();
        if poison_worker {
            assert!(
                catch_unwind(AssertUnwindSafe(|| {
                    let _guard = backend.inner.recovery_worker.lock().unwrap();
                    panic!("poison recovery worker inventory");
                }))
                .is_err()
            );
        }
        if poison_state {
            assert!(
                catch_unwind(AssertUnwindSafe(|| {
                    let mut state = backend.inner.state.lock().unwrap();
                    state.admitted = true;
                    state.status = StorageState::Ready;
                    panic!("poison admitted state");
                }))
                .is_err()
            );
        }
        let owner = crate::service_lifecycle::ManagementShutdown::new(backend.clone());
        let (unwinding, entered) = mpsc::channel();
        let (returned, completed) = mpsc::channel();
        let cleanup = std::thread::spawn(move || {
            let panic = catch_unwind(AssertUnwindSafe(move || {
                let _owner = owner;
                unwinding.send(()).unwrap();
                panic!("outer management cleanup unwind");
            }))
            .unwrap_err();
            returned
                .send(panic.downcast_ref::<&str>().copied())
                .unwrap();
        });
        entered.recv_timeout(DEADLINE).unwrap();
        assert_eq!(
            completed.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "owner cleanup must wait for the held native recovery worker"
        );
        release.send(()).unwrap();
        assert_eq!(
            completed.recv_timeout(DEADLINE).unwrap(),
            Some("outer management cleanup unwind")
        );
        cleanup.join().unwrap();
        outcome.recv_timeout(DEADLINE).unwrap().unwrap();
        assert_eq!(Arc::strong_count(&retained), 1);
        assert!(backend.inner.stopping.load(Ordering::SeqCst));
        assert_eq!(backend.inner.recovery_worker.is_poisoned(), poison_worker);
        assert_eq!(backend.inner.state.is_poisoned(), poison_state);
        let worker = backend
            .inner
            .recovery_worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(worker.is_none());
        drop(worker);
        let state = backend
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(!state.admitted);
        assert_eq!(state.status, StorageState::Unavailable);
        drop(state);
        let error = backend.drain_management().unwrap_err();
        let source = shutdown_error(&error);
        assert_eq!(source.recovery_worker_poisoned, poison_worker);
        assert_eq!(source.admission_state_poisoned, poison_state);
        assert!(source.worker_panic.is_none());
        assert_eq!(
            backend.inner.ensure_running().unwrap_err().code,
            storage_contract::ErrorCode::ServiceStopping
        );
    }
}

#[test]
fn recovery_worker_panic_and_poison_keep_concrete_sources_without_panicking_drain() {
    use std::error::Error;
    for poison_state in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let (backend, _) = open_service(
            &temporary.path().join("system"),
            &temporary.path().join("management"),
        )
        .unwrap();
        let inner = Arc::clone(&backend.inner);
        *backend.inner.recovery_worker.lock().unwrap() = Some(std::thread::spawn(move || {
            if poison_state {
                let _state = inner.state.lock().unwrap();
                panic!("injected recovery worker panic");
            }
            panic!("injected recovery worker panic");
        }));
        let error = backend.drain_management().unwrap_err();
        let shutdown = shutdown_error(&error);
        assert!(!shutdown.recovery_worker_poisoned);
        assert_eq!(shutdown.admission_state_poisoned, poison_state);
        let panic = shutdown
            .source()
            .unwrap()
            .downcast_ref::<RecoveryWorkerPanic>()
            .unwrap();
        assert_eq!(
            panic.message.as_deref(),
            Some("injected recovery worker panic")
        );
        assert!(backend.inner.recovery_worker.lock().unwrap().is_none());
        assert!(backend.inner.stopping.load(Ordering::SeqCst));
        assert_eq!(backend.inner.state.is_poisoned(), poison_state);
    }
}
