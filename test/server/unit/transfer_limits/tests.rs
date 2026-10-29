use super::*;
use futures::{StreamExt, stream};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn admission_enforces_the_single_capacity_and_releases_permits() {
    let limits = TransferLimits::default();
    assert_eq!(limits.slots, 2);
    let admission = Admission::new(&limits);
    let a = admission.acquire().unwrap();
    let b = admission.acquire().unwrap();
    assert_eq!(admission.acquire().err().unwrap().status, 429);
    drop(a);
    let c = admission.acquire().unwrap();
    assert_eq!(admission.acquire().err().unwrap().status, 429);
    drop((b, c));
    assert_eq!(*admission.active.lock().unwrap(), 0);
}

#[test]
fn permit_drop_during_unwind_preserves_poisoned_admission_count() {
    let admission = Admission::new(&TransferLimits::default());
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _permit = admission.acquire().unwrap();
        let _guard = admission.active.lock().unwrap();
        panic!("permit owner failed while holding admission accounting");
    }));
    assert!(panic.is_err());
    assert!(admission.active.is_poisoned());
    assert_eq!(*admission.active.lock().err().unwrap().into_inner(), 1);
}

#[test]
fn invalid_limits_cannot_disable_capacity_or_deadlines() {
    for limits in [
        TransferLimits {
            slots: 0,
            ..TransferLimits::default()
        },
        TransferLimits {
            idle_timeout: Duration::ZERO,
            ..TransferLimits::default()
        },
        TransferLimits {
            total_timeout: Duration::from_secs(30),
            ..TransferLimits::default()
        },
        TransferLimits {
            total_timeout: Duration::MAX,
            ..TransferLimits::default()
        },
    ] {
        assert!(limits.validate().is_err());
    }
    assert!(TransferLimits::default().validate().is_ok());
}

struct Lease(Arc<AtomicUsize>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn short_limits() -> TransferLimits {
    TransferLimits {
        idle_timeout: Duration::from_millis(30),
        total_timeout: Duration::from_secs(2),
        ..TransferLimits::default()
    }
}
async fn wait_for_drop(dropped: &AtomicUsize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while dropped.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unpolled_body_expires_and_cannot_resume_old_stream() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let lease = Lease(dropped.clone());
    let source = stream::repeat_with(move || {
        let _lease = &lease;
        Ok(bytes::Bytes::from_static(b"x"))
    });
    let limits = short_limits();
    let mut body = BoundedStream::new(source, &limits, Instant::now() + limits.total_timeout);
    wait_for_drop(&dropped).await;
    assert_eq!(
        body.next().await.unwrap().unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
    assert!(body.next().await.is_none());
    drop(body);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn total_deadline_expires_even_while_chunks_keep_progressing() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let lease = Lease(dropped.clone());
    let source = stream::repeat_with(move || {
        let _lease = &lease;
        Ok(bytes::Bytes::from_static(b"x"))
    });
    let limits = TransferLimits {
        idle_timeout: Duration::from_secs(2),
        ..short_limits()
    };
    let mut body = BoundedStream::new(source, &limits, Instant::now() + Duration::from_millis(40));
    while body.next().await.unwrap().is_ok() {
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    assert!(body.next().await.is_none());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn timeout_does_not_release_a_running_blocking_read() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let lease = Lease(dropped.clone());
    let limits = short_limits();
    let admission = Admission::new(&limits);
    let permit = admission.acquire().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();
    let source = stream::once(async move {
        tokio::task::spawn_blocking(move || {
            let _ownership = (lease, permit);
            started_tx.send(()).unwrap();
            finish_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"x"))
        })
        .await
        .unwrap()
    });
    let mut body = BoundedStream::new(source, &limits, Instant::now() + limits.total_timeout);
    let pending = tokio::spawn(async move {
        assert_eq!(
            body.next().await.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(body.next().await.is_none());
        body
    });
    started_rx.await.unwrap();
    let body = pending.await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(*admission.active.lock().unwrap(), 1);
    finish_tx.send(()).unwrap();
    wait_for_drop(&dropped).await;
    assert_eq!(*admission.active.lock().unwrap(), 0);
    drop(body);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ready_stream_cannot_starve_total_or_empty_frame_idle_deadline() {
    // The current-thread runtime cannot run the watchdog while this loop
    // consumes always-ready chunks without yielding. poll_next must enforce
    // the deadline itself, including when empty chunks supply no progress.
    for empty in [false, true] {
        let limits = TransferLimits {
            idle_timeout: if empty {
                Duration::from_millis(3)
            } else {
                Duration::from_secs(1)
            },
            ..TransferLimits::default()
        };
        let deadline = Instant::now()
            + if empty {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(3)
            };
        let source = stream::repeat_with(move || {
            Ok(if empty {
                bytes::Bytes::new()
            } else {
                bytes::Bytes::from_static(b"x")
            })
        });
        let mut body = BoundedStream::new(source, &limits, deadline);
        loop {
            if let Err(error) = body.next().await.unwrap() {
                assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
                break;
            }
        }
        assert!(body.next().await.is_none());
    }
}

#[tokio::test]
async fn explicit_drop_releases_idle_stream_without_waiting_for_timer() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let lease = Lease(dropped.clone());
    let source = stream::pending::<Result<bytes::Bytes, std::io::Error>>().map(move |chunk| {
        let _lease = &lease;
        chunk
    });
    let limits = short_limits();
    let body = BoundedStream::new(source, &limits, Instant::now() + limits.total_timeout);
    drop(body);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn poisoned_body_drop_during_unwind_aborts_timer_and_releases_stream_owner() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let lease = Lease(Arc::clone(&dropped));
    let source = stream::poll_fn(
        move |_| -> Poll<Option<Result<bytes::Bytes, std::io::Error>>> {
            let _lease = &lease;
            panic!("source stream failed while being polled");
        },
    );
    let limits = TransferLimits::default();
    let body = BoundedStream::new(source, &limits, Instant::now() + limits.total_timeout);
    let state = Arc::clone(&body.state);
    // Keep the watch channel open: cancellation must not depend on its sender
    // disappearing, and the normal idle deadline lies beyond wait_for_drop.
    let progress = body.progress.clone();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut body = body;
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        let _ = body.poll_next_unpin(&mut context);
    }));
    assert!(panic.is_err());
    assert!(state.is_poisoned());
    assert!(state.lock().err().unwrap().into_inner().stream.is_some());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(state);
    wait_for_drop(&dropped).await;
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    drop(progress);
}
