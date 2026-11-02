#[path = "semaphore-raw-transfer-support.rs"]
mod support;

use futures_intrusive::sync::{
    GenericSemaphore, GenericSharedSemaphore, SharedSemaphore, SharedSemaphoreReleaser,
};
use std::future::Future;
use std::task::{Context, Poll, Waker};
use support::{SyncOnlyRaw, raw_drop_count};

fn require_sync<T: Sync>() {}
fn require_send<T: Send>(value: T) -> T {
    value
}

// Every call starts with an available permit. No waiter or waker is queued.
fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("invariant: the control has enough initial permits"),
    }
}

fn native_owned_is_send_and_shared() {
    require_sync::<SharedSemaphore>();
    require_sync::<SharedSemaphoreReleaser>();
    let semaphore = SharedSemaphore::new(true, 1);
    let future = require_send(semaphore.acquire(1));
    drop(semaphore);
    std::thread::spawn(move || {
        let releaser = require_send(ready(future));
        drop(releaser);
    })
    .join()
    .expect("invariant: native owned acquire control completes");

    let semaphore = SharedSemaphore::new(true, 1);
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let clone = semaphore.clone();
                let releaser = clone
                    .try_acquire(1)
                    .expect("invariant: fresh shared control has a permit");
                drop(releaser);
            })
            .join()
            .expect("invariant: native shared clone control completes");
    });
    assert_eq!(semaphore.permits(), 1);
}

fn sync_only_owners_remain_local() {
    require_sync::<SyncOnlyRaw>();
    let before = raw_drop_count();

    let semaphore = GenericSharedSemaphore::<SyncOnlyRaw>::new(true, 1);
    let clone: GenericSharedSemaphore<SyncOnlyRaw> = {
        let borrowed = &semaphore;
        borrowed.clone()
    };
    drop(semaphore);
    assert_eq!(raw_drop_count(), before);
    drop(clone);
    assert_eq!(raw_drop_count(), before + 1);

    let semaphore = GenericSharedSemaphore::<SyncOnlyRaw>::new(true, 1);
    let future = semaphore.acquire(1);
    drop(semaphore);
    assert_eq!(raw_drop_count(), before + 1);
    drop(future);
    assert_eq!(raw_drop_count(), before + 2);

    let semaphore = GenericSharedSemaphore::<SyncOnlyRaw>::new(true, 1);
    let future = semaphore.acquire(1);
    drop(semaphore);
    let releaser = ready(future);
    assert_eq!(raw_drop_count(), before + 2);
    drop(releaser);
    assert_eq!(raw_drop_count(), before + 3);
}

fn borrowed_sync_owner_needs_no_send() {
    require_sync::<GenericSemaphore<SyncOnlyRaw>>();
    let before = raw_drop_count();
    let semaphore = GenericSemaphore::<SyncOnlyRaw>::new(true, 1);
    let future = require_send(semaphore.acquire(1));
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let releaser = require_send(ready(future));
                drop(releaser);
            })
            .join()
            .expect("invariant: borrowed acquire control completes");
    });
    assert_eq!(semaphore.permits(), 1);
    assert_eq!(raw_drop_count(), before);
    drop(semaphore);
    assert_eq!(raw_drop_count(), before + 1);
}

fn main() {
    native_owned_is_send_and_shared();
    sync_only_owners_remain_local();
    borrowed_sync_owner_needs_no_send();
    println!("SEMAPHORE_RAW_TRANSFER_CONTROLS_COMPLETE");
}
