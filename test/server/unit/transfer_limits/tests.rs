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
    assert_eq!(*admission.active.lock().unwrap(), Some(0));
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
    assert_eq!(*admission.active.lock().unwrap(), Some(1));
    finish_tx.send(()).unwrap();
    wait_for_drop(&dropped).await;
    assert_eq!(*admission.active.lock().unwrap(), Some(0));
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

#[test]
fn permit_poisoned_drop_during_unwind_preserves_fence() {
    let admission = Admission::new(&TransferLimits::default());
    let permit = admission.acquire().unwrap();
    let _ = std::panic::catch_unwind(|| {
        let _guard = admission.active.lock().unwrap();
        panic!("poison");
    });
    assert!(
        std::panic::catch_unwind(|| {
            let _permit = permit;
            panic!("outer unwind");
        })
        .is_err()
    );
    assert!(admission.active.is_poisoned());
    assert_eq!(*admission.active.lock().unwrap_err().into_inner(), Some(1));
    assert!(std::panic::catch_unwind(|| admission.acquire()).is_err());
}

#[test]
fn invalid_permit_count_fences_new_admission() {
    let admission = Admission::new(&TransferLimits::default());
    let permit = admission.acquire().unwrap();
    *admission.active.lock().unwrap() = Some(0);
    drop(permit);
    assert_eq!(
        admission.acquire().err().unwrap().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
}

#[tokio::test]
async fn panicking_stream_can_unwind_through_bounded_stream_drop() {
    struct Panics(Arc<AtomicUsize>);
    impl Stream for Panics {
        type Item = Result<bytes::Bytes, std::io::Error>;
        fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            panic!("stream poll failed");
        }
    }
    impl Drop for Panics {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let limits = TransferLimits::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut body = BoundedStream::new(
            Panics(dropped.clone()),
            &limits,
            Instant::now() + limits.total_timeout,
        );
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        let _ = Pin::new(&mut body).poll_next(&mut context);
    }));
    assert!(result.is_err());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
