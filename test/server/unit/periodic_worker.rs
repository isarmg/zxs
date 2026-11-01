use super::*;
use std::sync::Arc;
const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn concurrent_drains_wait_for_active_sample_and_release_resources() {
    let worker = Arc::new(PeriodicWorker::new());
    let resource = Arc::new(());
    let retained = Arc::clone(&resource);
    let (enter, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    worker
        .start(
            thread::Builder::new(),
            Duration::ZERO,
            DEADLINE,
            move || {
                let _keep = &retained;
                enter.send(()).unwrap();
                released.recv_timeout(DEADLINE).unwrap();
            },
        )
        .unwrap();
    entered.recv_timeout(DEADLINE).unwrap();
    worker.stop();
    worker.stop();
    let (done, completed) = mpsc::channel();
    let joins: Vec<_> = (0..2)
        .map(|_| {
            let worker = Arc::clone(&worker);
            let done = done.clone();
            thread::spawn(move || done.send(worker.drain()).unwrap())
        })
        .collect();
    assert!(matches!(
        completed.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(Arc::strong_count(&resource), 2);
    release.send(()).unwrap();
    for join in joins {
        completed.recv_timeout(DEADLINE).unwrap().unwrap();
        join.join().unwrap();
    }
    assert_eq!(Arc::strong_count(&resource), 1);
    worker.drain().unwrap();
}

#[test]
fn stop_wakes_idle_worker_and_fences_start() {
    let worker = PeriodicWorker::new();
    worker
        .start(
            thread::Builder::new(),
            Duration::from_secs(3600),
            DEADLINE,
            || panic!("must not sample"),
        )
        .unwrap();
    worker.stop();
    worker.drain().unwrap();
    assert!(
        worker
            .start(thread::Builder::new(), Duration::ZERO, DEADLINE, || {})
            .is_err()
    );
}

#[test]
fn panic_is_retained_across_repeated_drain_without_panicking_drop() {
    let worker = PeriodicWorker::new();
    let (entered, entry) = mpsc::channel();
    worker
        .start(
            thread::Builder::new(),
            Duration::ZERO,
            DEADLINE,
            move || {
                entered.send(()).unwrap();
                panic!("observer panic");
            },
        )
        .unwrap();
    entry.recv_timeout(DEADLINE).unwrap();
    for _ in 0..2 {
        let error = worker.drain().unwrap_err();
        let crate::http_boundary::ApiErrorSource::Io(source) = error.source.as_deref().unwrap()
        else {
            panic!("expected IO source")
        };
        assert_eq!(
            source
                .get_ref()
                .unwrap()
                .downcast_ref::<WorkerPanic>()
                .unwrap()
                .message
                .as_deref(),
            Some("observer panic")
        );
    }
    drop(worker);
}

#[test]
fn stopped_unstarted_owner_rejects_late_start_and_drops_captured_resources() {
    let worker = PeriodicWorker::new();
    worker.stop();
    let resource = Arc::new(());
    let retained = Arc::clone(&resource);
    assert!(
        worker
            .start(
                thread::Builder::new(),
                Duration::ZERO,
                DEADLINE,
                move || {
                    let _ = &retained;
                }
            )
            .is_err()
    );
    assert_eq!(Arc::strong_count(&resource), 1);
    worker.drain().unwrap();
}

#[test]
fn spawn_failure_releases_capture_and_drain_still_closes_owner() {
    let worker = PeriodicWorker::new();
    let resource = Arc::new(());
    let retained = Arc::clone(&resource);
    let error = worker
        .start(
            thread::Builder::new().stack_size(usize::MAX),
            Duration::ZERO,
            DEADLINE,
            move || {
                let _keep = &retained;
            },
        )
        .unwrap_err();
    assert!(error.raw_os_error().is_some());
    assert_eq!(Arc::strong_count(&resource), 1);
    worker.drain().unwrap();
}
