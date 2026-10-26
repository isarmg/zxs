use super::*;
use crate::{ApiError, Backend, BackendAction, Content, auth::Identity};
use json_codec::Value;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

struct WriterProbe {
    stopped: AtomicBool,
    failure: Option<&'static str>,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    done: Mutex<Option<oneshot::Sender<()>>>,
}
impl Backend for WriterProbe {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not query operations")
    }
    fn commit_workspace(
        &self,
        _: &crate::auth::Identity,
        _: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not commit workspaces")
    }

    fn snapshot_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshots")
    }

    fn lookup_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
        _: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshot lookups")
    }

    fn stop_management(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        assert!(self.stopped.load(Ordering::SeqCst));
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        self.release.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        self.done.lock().unwrap().take().unwrap().send(()).unwrap();
        match self.failure {
            Some(message) => Err(ApiError::io(io::Error::other(message))),
            None => Ok(()),
        }
    }
    fn call(&self, _: BackendAction, _: &Identity, _: Value) -> Result<Value, ApiError> {
        Err(ApiError::unsupported())
    }
    fn upload(
        &self,
        _: &Identity,
        _: &str,
        _: &str,
        _: &name_policy::PortablePath,
        _: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        Err(ApiError::unsupported())
    }
    fn content(&self, _: &Identity, _: &str) -> Result<Content, ApiError> {
        Err(ApiError::unsupported())
    }
}
fn writer() -> (
    Arc<WriterProbe>,
    oneshot::Receiver<()>,
    mpsc::Sender<()>,
    oneshot::Receiver<()>,
) {
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let (done, finished) = oneshot::channel();
    (
        Arc::new(WriterProbe {
            stopped: AtomicBool::new(false),
            failure: None,
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(released),
            done: Mutex::new(Some(done)),
        }),
        started,
        release,
        finished,
    )
}

#[tokio::test]
async fn shutdown_waits_for_native_writer_after_http_has_drained() {
    let (backend, started, release, finished) = writer();
    let (stop, shutdown) = oneshot::channel();
    let server = tokio::spawn(run_with_management(
        |stopped| async move {
            let _ = stopped.await;
            Ok(())
        },
        pending(),
        async move {
            let _ = shutdown.await;
        },
        ManagementShutdown::new(backend.clone()),
    ));
    stop.send(()).unwrap();
    notice(started).await;
    assert!(backend.stopped.load(Ordering::SeqCst));
    assert!(!server.is_finished());
    release.send(()).unwrap();
    notice(finished).await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn http_error_still_drains_native_writer_before_returning_error() {
    let (backend, started, release, finished) = writer();
    let server = tokio::spawn(run_with_management(
        |_| async { Err(io::Error::other("HTTP failed")) },
        pending(),
        pending(),
        ManagementShutdown::new(backend),
    ));
    notice(started).await;
    assert!(!server.is_finished());
    release.send(()).unwrap();
    notice(finished).await;
    assert_eq!(
        server.await.unwrap().unwrap_err().to_string(),
        "HTTP failed"
    );
}

#[tokio::test]
async fn cancelled_service_keeps_native_drain_in_runtime_owned_blocking_work() {
    let (backend, started, release, finished) = writer();
    let (bridge, bridge_started, bridge_dropped) = bridge();
    let server = tokio::spawn(run_with_management(
        |_| pending(),
        bridge,
        pending(),
        ManagementShutdown::new(backend.clone()),
    ));
    notice(bridge_started).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    notice(bridge_dropped).await;
    notice(started).await;
    assert!(backend.stopped.load(Ordering::SeqCst));
    release.send(()).unwrap();
    notice(finished).await;
}

#[test]
fn guard_drop_without_runtime_drains_synchronously_without_panicking() {
    let (backend, mut started, release, mut finished) = writer();
    release.send(()).unwrap();
    drop(ManagementShutdown::new(backend.clone()));
    assert!(backend.stopped.load(Ordering::SeqCst));
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());
}

#[test]
fn discarded_queued_blocking_closure_still_owns_native_drain() {
    let (backend, mut started, release, mut finished) = writer();
    backend.stop_management();
    release.send(()).unwrap();
    let drain = NativeDrain(Some(backend));
    // Model cancellation before Tokio polls a queued blocking closure. Merely
    // capturing the backend Arc would detach its native writer at this point.
    let queued = move || drain.run();
    drop(queued);
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());
}

#[test]
fn closed_runtime_rejecting_blocking_enqueue_still_drains_native_writer() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();
    drop(runtime);
    let _entered_runtime = handle.enter();
    let (backend, mut started, release, mut finished) = writer();
    release.send(()).unwrap();
    // try_current succeeds, but the stopped blocking pool discards this job.
    drop(ManagementShutdown::new(backend.clone()));
    assert!(backend.stopped.load(Ordering::SeqCst));
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());
}

#[test]
fn failed_drain_is_returned_or_reported_once_after_cancellation() {
    use std::io::{Read, Seek};
    let mut capture = tempfile::tempfile().unwrap();
    let subscriber = tracing::collector::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(Arc::new(capture.try_clone().unwrap()))
        .finish();
    // Detached blocking outputs may be dropped on any runtime worker.
    tracing::subscriber::set_global_default(subscriber).unwrap();

    for (cancel, marker) in [
        (false, "lifecycle-returned-drain-failure"),
        (true, "lifecycle-detached-drain-failure"),
    ] {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut backend, started, release, finished) = writer();
        Arc::get_mut(&mut backend).unwrap().failure = Some(marker);
        runtime.block_on(async {
            let mut management = ManagementShutdown::new(backend);
            let task = tokio::spawn(async move { management.finish().await });
            notice(started).await;
            if cancel {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                release.send(()).unwrap();
            } else {
                release.send(()).unwrap();
                let error = task.await.unwrap().unwrap_err();
                let source = error.get_ref().unwrap().downcast_ref::<ApiError>().unwrap();
                assert!(format!("{source:?}").contains(marker));
            }
            notice(finished).await;
        });
        // Runtime teardown waits for the detached native drain and its output.
        drop(runtime);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let (occupied, occupied_rx) = mpsc::channel();
    let (unblock, unblock_rx) = mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        occupied.send(()).unwrap();
        unblock_rx.recv_timeout(DEADLINE).unwrap();
    });
    occupied_rx.recv_timeout(DEADLINE).unwrap();
    let (mut backend, mut started, release, mut finished) = writer();
    Arc::get_mut(&mut backend).unwrap().failure = Some("lifecycle-queued-drain-failure");
    release.send(()).unwrap();
    {
        let _entered = runtime.enter();
        let mut management = ManagementShutdown::new(backend);
        management.begin();
        management.drain.as_ref().unwrap().abort();
        drop(management);
    }
    unblock.send(()).unwrap();
    runtime.block_on(blocker).unwrap();
    drop(runtime);
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());

    capture.rewind().unwrap();
    let mut logged = String::new();
    capture.read_to_string(&mut logged).unwrap();
    assert_eq!(
        logged.matches("lifecycle-returned-drain-failure").count(),
        0,
        "{logged}"
    );
    assert_eq!(
        logged.matches("lifecycle-detached-drain-failure").count(),
        1,
        "{logged}"
    );
    assert_eq!(
        logged.matches("lifecycle-queued-drain-failure").count(),
        1,
        "{logged}"
    );
}

#[test]
fn simultaneous_http_and_management_failures_keep_both_concrete_causes() {
    let error = combine_shutdown_results(
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "HTTP failure",
        )),
        Err(io::Error::other(ApiError::io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "management failure",
        )))),
    )
    .unwrap_err();
    let combined = error
        .get_ref()
        .unwrap()
        .downcast_ref::<HttpAndManagementError>()
        .unwrap();
    assert_eq!(combined.http.kind(), io::ErrorKind::ConnectionReset);
    assert!(combined.management.get_ref().unwrap().is::<ApiError>());
    assert!(format!("{combined:?}").contains("management failure"));
}
