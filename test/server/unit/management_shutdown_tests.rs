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
