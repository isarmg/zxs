use super::*;
use std::{io::Read, time::Duration};

const DEADLINE: Duration = Duration::from_secs(5);

struct IdleConnection {
    client: UnixStream,
    outcome: std::sync::mpsc::Receiver<Result<(), io::ErrorKind>>,
}
impl IdleConnection {
    fn assert_stopped(mut self) {
        assert_eq!(self.client.read(&mut [0]).unwrap(), 0);
        assert_eq!(
            self.outcome.recv_timeout(DEADLINE).unwrap(),
            Err(io::ErrorKind::UnexpectedEof),
            "the worker must report actual established-session EOF to the test owner"
        );
    }
}

fn idle(registry: &ConnectionRegistry, id: u64) -> IdleConnection {
    let (client, server) = UnixStream::pair().unwrap();
    client.set_read_timeout(Some(DEADLINE)).unwrap();
    let connections = {
        let state = registry.0.lock().unwrap();
        Arc::clone(state.as_ref().unwrap().connections.as_ref().unwrap())
    };
    connections
        .lock()
        .unwrap()
        .insert(id, server.try_clone().unwrap());
    let (report, outcome) = std::sync::mpsc::channel();
    registry
        .spawn(move || {
            let result = FrameStream::new(server)
                .and_then(|mut stream| {
                    stream.receive_with_mode::<Request>(
                        smb_protocol::protocol::ReadMode::EstablishedSession,
                    )
                })
                .map(|_| ())
                .map_err(|error| error.kind());
            closing_connections(&connections).remove(&id);
            report.send(result).map_err(io::Error::other)
        })
        .unwrap();
    IdleConnection { client, outcome }
}

struct HeldWork {
    release: std::sync::mpsc::Sender<()>,
    outcome: std::sync::mpsc::Receiver<Result<(), std::sync::mpsc::RecvTimeoutError>>,
}
fn held_work(registry: &ConnectionRegistry, marker: &Arc<()>) -> HeldWork {
    let retained = Arc::clone(marker);
    let (started, running) = std::sync::mpsc::channel();
    let (release, work) = std::sync::mpsc::channel();
    let (report, outcome) = std::sync::mpsc::channel();
    registry
        .spawn(move || {
            let _retained = retained;
            started.send(()).map_err(io::Error::other)?;
            let result = work.recv_timeout(DEADLINE);
            report.send(result).map_err(io::Error::other)
        })
        .unwrap();
    running.recv_timeout(DEADLINE).unwrap();
    HeldWork { release, outcome }
}

#[test]
fn idle_connections_do_not_occupy_shared_blocking_pool() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut shutdown = ConnectionShutdown::new();
        let registry = shutdown.registry();
        let client = idle(&registry, 1);
        let result = tokio::time::timeout(DEADLINE, tokio::task::spawn_blocking(|| 42))
            .await
            .expect("idle SMB must not pin the sole HTTP blocking worker")
            .unwrap();
        assert_eq!(result, 42);
        tokio::time::timeout(DEADLINE, shutdown.finish())
            .await
            .unwrap()
            .unwrap();
        client.assert_stopped();
        assert!(registry.0.lock().unwrap().is_none());
    });
}

#[tokio::test]
async fn thread_and_handle_capacity_is_bounded_and_shutdown_joins_every_worker() {
    let mut shutdown = ConnectionShutdown::new();
    let registry = shutdown.registry();
    let clients = (0..MAX_CONNECTIONS)
        .map(|id| idle(&registry, id as u64))
        .collect::<Vec<_>>();
    let admission = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = Arc::clone(&admission).try_acquire_owned().unwrap();
    let (ran, rejected) = std::sync::mpsc::channel();
    let error = registry
        .spawn(move || {
            drop(permit);
            ran.send(()).map_err(io::Error::other)
        })
        .unwrap_err();
    assert_eq!(
        admission.available_permits(),
        1,
        "rejected closure must return its admission slot"
    );
    assert_eq!(
        rejected.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected),
        "capacity rejection must drop, never execute, the submitted closure"
    );
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(
        registry.0.lock().unwrap().as_ref().unwrap().threads.len(),
        MAX_CONNECTIONS
    );
    tokio::time::timeout(DEADLINE, shutdown.finish())
        .await
        .unwrap()
        .unwrap();
    for client in clients {
        client.assert_stopped();
    }
    assert_eq!(
        registry.spawn(|| Ok(())).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}

#[tokio::test]
async fn shutdown_waits_for_admitted_work_without_blocking_async_timers() {
    let mut shutdown = ConnectionShutdown::new();
    let registry = shutdown.registry();
    let marker = Arc::new(());
    let held = held_work(&registry, &marker);
    shutdown.begin();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), shutdown.finish())
            .await
            .is_err()
    );
    assert_eq!(
        Arc::strong_count(&marker),
        2,
        "active operation must remain owned"
    );
    held.release.send(()).unwrap();
    shutdown.finish().await.unwrap();
    held.outcome.recv_timeout(DEADLINE).unwrap().unwrap();
    assert_eq!(Arc::strong_count(&marker), 1);
}

#[test]
fn owner_drop_outside_runtime_wakes_socket_and_joins() {
    let shutdown = ConnectionShutdown::new();
    let registry = shutdown.registry();
    let client = idle(&registry, 1);
    drop(shutdown);
    client.assert_stopped();
    assert!(registry.0.lock().unwrap().is_none());
}

#[test]
fn cancelled_owner_is_drained_by_runtime_teardown() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let marker = Arc::new(());
    let held = runtime.block_on(async {
        let shutdown = ConnectionShutdown::new();
        let registry = shutdown.registry();
        let client = idle(&registry, 1);
        let held = held_work(&registry, &marker);
        let task = tokio::spawn(async move {
            let _shutdown = shutdown;
            std::future::pending::<()>().await;
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        client.assert_stopped();
        assert!(registry.0.lock().unwrap().is_none());
        assert_eq!(
            Arc::strong_count(&marker),
            2,
            "native work is still held after owner cancellation"
        );
        held
    });
    let (entering, entered) = std::sync::mpsc::channel();
    let (finished, completion) = std::sync::mpsc::channel();
    let teardown = std::thread::spawn(move || {
        entering.send(()).unwrap();
        drop(runtime);
        finished.send(()).unwrap();
    });
    entered.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        completion.recv_timeout(Duration::from_millis(20)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout),
        "runtime teardown must wait for the explicitly held native worker"
    );
    held.release.send(()).unwrap();
    completion.recv_timeout(DEADLINE).unwrap();
    teardown.join().unwrap();
    held.outcome.recv_timeout(DEADLINE).unwrap().unwrap();
    assert_eq!(Arc::strong_count(&marker), 1);
}

#[test]
fn discarded_shutdown_job_still_owns_socket_wakeup_and_native_joins() {
    let shutdown = ConnectionShutdown::new();
    let registry = shutdown.registry();
    let client = idle(&registry, 1);
    let workers = registry.0.lock().unwrap().take().unwrap();
    // Model a queued blocking closure discarded before it starts. Ownership,
    // rather than execution of the closure body, must enforce native joins.
    let queued = move || drop(workers);
    drop(queued);
    client.assert_stopped();
}

#[tokio::test]
async fn worker_panic_is_joined_without_poisoning_shutdown_owner() {
    let mut shutdown = ConnectionShutdown::new();
    shutdown
        .registry()
        .spawn(|| panic!("injected worker panic"))
        .unwrap();
    shutdown.finish().await.unwrap();
    assert!(shutdown.registry.0.lock().unwrap().is_none());
}

#[test]
fn unwinding_owner_closes_poisoned_registries_and_joins_without_second_panic() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    for poison_workers in [false, true] {
        let shutdown = ConnectionShutdown::new();
        let registry = shutdown.registry();
        let client = idle(&registry, 1);
        let connections = Arc::clone(
            registry
                .0
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .connections
                .as_ref()
                .unwrap(),
        );
        let retained = Arc::new(());
        let held = held_work(&registry, &retained);
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                if poison_workers {
                    let _guard = registry.0.lock().unwrap();
                    panic!("poison native worker inventory");
                } else {
                    let _guard = connections.lock().unwrap();
                    panic!("poison socket inventory");
                }
            }))
            .is_err()
        );
        held.release.send(()).unwrap();
        assert!(
            catch_unwind(AssertUnwindSafe(move || {
                let _shutdown = shutdown;
                panic!("owner is dropped during unwind");
            }))
            .is_err()
        );
        client.assert_stopped();
        held.outcome.recv_timeout(DEADLINE).unwrap().unwrap();
        assert_eq!(
            Arc::strong_count(&retained),
            1,
            "all native workers must be joined"
        );
        // Cleanup must not make poisoned state usable again.
        assert_eq!(registry.0.is_poisoned(), poison_workers);
        assert_eq!(connections.is_poisoned(), !poison_workers);
        let state = match registry.0.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(state.is_none());
    }
}

#[tokio::test]
async fn completed_connection_handles_are_reaped_before_the_next_admission() {
    let mut shutdown = ConnectionShutdown::new();
    let registry = shutdown.registry();
    for id in 0..3 {
        let connection = idle(&registry, id);
        assert_eq!(
            registry.0.lock().unwrap().as_ref().unwrap().threads.len(),
            1,
            "old completed handles must not accumulate across connections"
        );
        connection
            .client
            .shutdown(std::net::Shutdown::Both)
            .unwrap();
        connection.assert_stopped();
        // The result channel proves receive/cleanup completed. is_finished
        // observes the remaining native-thread return before the next admit;
        // no arbitrary sleep or production polling hook is involved.
        tokio::time::timeout(DEADLINE, async {
            loop {
                if registry.0.lock().unwrap().as_ref().unwrap().threads[0].is_finished() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    shutdown.finish().await.unwrap();
}

#[test]
fn enqueue_panic_after_queueing_during_unwind_falls_back_and_later_queue_drain_is_safe() {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::atomic::{AtomicUsize, Ordering},
    };
    let attempts = Arc::new(AtomicUsize::new(0));
    let thread_attempts = Arc::clone(&attempts);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name_fn(move || {
            // This callback is inside the real runtime spawn_thread path,
            // after the drain closure has already entered the blocking queue.
            if thread_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("injected blocking-thread creation boundary failure");
            }
            "smb-drain-test".into()
        })
        .build()
        .unwrap();
    runtime.block_on(async {
        let shutdown = ConnectionShutdown::new();
        let registry = shutdown.registry();
        let client = idle(&registry, 1);
        let marker = Arc::new(());
        let held = held_work(&registry, &marker);
        held.release.send(()).unwrap();
        let panic = catch_unwind(AssertUnwindSafe(move || {
            let _shutdown = shutdown;
            panic!("outer owner unwind");
        }))
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"outer owner unwind"));
        client.assert_stopped();
        held.outcome.recv_timeout(DEADLINE).unwrap().unwrap();
        assert_eq!(
            Arc::strong_count(&marker),
            1,
            "fallback must complete native joins"
        );
        assert!(registry.0.lock().unwrap().is_none());
        // Starting a real blocking thread now runs the previously queued
        // closure as well as this job. Its shared obligation is already empty.
        assert_eq!(tokio::task::spawn_blocking(|| 42).await.unwrap(), 42);
    });
    drop(runtime);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[test]
fn concurrent_drain_consumers_both_wait_for_actual_native_join() {
    let shutdown = ConnectionShutdown::new();
    let registry = shutdown.registry();
    let marker = Arc::new(());
    let held = held_work(&registry, &marker);
    let workers = registry.0.lock().unwrap().take().unwrap();
    let handoff = Arc::new(Mutex::new(Some(workers)));
    let ready = Arc::new(std::sync::Barrier::new(3));
    let (finished, completion) = std::sync::mpsc::channel();
    let consumers = (0..2)
        .map(|_| {
            let drain = WorkerDrain(Arc::clone(&handoff));
            let ready = Arc::clone(&ready);
            let finished = finished.clone();
            std::thread::spawn(move || {
                ready.wait();
                drop(drain);
                finished.send(()).unwrap();
            })
        })
        .collect::<Vec<_>>();
    ready.wait();
    assert_eq!(
        completion.recv_timeout(Duration::from_millis(20)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout),
        "even the losing take-once consumer must wait for the winning consumer's join"
    );
    held.release.send(()).unwrap();
    for consumer in consumers {
        completion.recv_timeout(DEADLINE).unwrap();
        consumer.join().unwrap();
    }
    held.outcome.recv_timeout(DEADLINE).unwrap().unwrap();
    assert_eq!(Arc::strong_count(&marker), 1);
    assert!(handoff.lock().unwrap().is_none());
}
