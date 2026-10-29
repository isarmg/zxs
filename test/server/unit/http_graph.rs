use super::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn graph_read_control_expires_and_owner_drop_cancels_without_releasing_permits() {
    let expired = GraphReadControl::until(Instant::now());
    assert_eq!(
        expired.checkpoint().unwrap_err().code(),
        storage_contract::ErrorCode::RequestTimeout
    );
    let control = GraphReadControl::until(Instant::now() + Duration::from_secs(30));
    let wait = control.wait_owner();
    control.checkpoint().unwrap();
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = slots.clone().try_acquire_owned().unwrap();
    drop(wait);
    assert_eq!(
        control.checkpoint().unwrap_err().code(),
        storage_contract::ErrorCode::RequestTimeout
    );
    assert_eq!(slots.available_permits(), 0);
    drop(permit);
    assert_eq!(slots.available_permits(), 1);
}

#[tokio::test]
async fn cancelled_wait_keeps_scan_permit_until_blocking_worker_observes_stop() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = slots.clone().try_acquire_owned().unwrap();
    let control = GraphReadControl::until(Instant::now() + Duration::from_secs(30));
    let wait = control.wait_owner();
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (finished, finish) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(async move {
        let _wait = wait;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            started.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            let error = control.checkpoint().unwrap_err();
            finished.send(error.code()).unwrap();
        })
        .await
        .unwrap();
    });
    start.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert_eq!(slots.available_permits(), 0);
    release.send(()).unwrap();
    assert_eq!(
        finish.await.unwrap(),
        storage_contract::ErrorCode::RequestTimeout
    );
    let _released = tokio::time::timeout(Duration::from_secs(5), slots.clone().acquire_owned())
        .await
        .unwrap()
        .unwrap();
}
