// Compile-only: never execute a cross-thread negative case.
#[path = "semaphore-raw-transfer-support.rs"]
mod support;

use futures_intrusive::sync::{GenericSharedSemaphore, GenericSharedSemaphoreAcquireFuture};
use support::{SyncOnlyRaw, raw_drop_count};

fn require_sync<T: Sync>() {}
fn require_send<T: Send>(_: T) {}

fn main() {
    require_sync::<SyncOnlyRaw>();
    let before = raw_drop_count();
    let semaphore = GenericSharedSemaphore::<SyncOnlyRaw>::new(true, 1);
    let future: GenericSharedSemaphoreAcquireFuture<SyncOnlyRaw> = semaphore.acquire(1);
    drop(semaphore);
    assert_eq!(raw_drop_count(), before);
    require_send(future);
}
