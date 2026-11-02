// Compile-only: this pins the shared owner's distinct Sync contract.
// No phantom-only admission is an executed cross-thread destructor witness.
#[path = "semaphore-raw-transfer-support.rs"]
mod support;

use futures_intrusive::sync::GenericSharedSemaphore;
use support::{SyncOnlyRaw, raw_drop_count};

fn require_sync<T: Sync>() {}

fn main() {
    require_sync::<SyncOnlyRaw>();
    let before = raw_drop_count();
    let semaphore = GenericSharedSemaphore::<SyncOnlyRaw>::new(true, 1);
    let clone: GenericSharedSemaphore<SyncOnlyRaw> = {
        let borrowed = &semaphore;
        borrowed.clone()
    };
    drop(semaphore);
    assert_eq!(raw_drop_count(), before);
    require_sync::<GenericSharedSemaphore<SyncOnlyRaw>>();
    drop(clone);
    assert_eq!(raw_drop_count(), before + 1);
}
