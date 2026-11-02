// Compile-only: the candidate must reject LocalRaw's Cell with E0277 for Sync.
// Never share this guard with a thread or execute a cross-thread negative case.
#[path = "upgradable-raw-support.rs"]
mod support;

use support::LocalRaw;
use synchronization::api::RwLock;

fn require_sync<T: ?Sized + Sync>(_: &T) {}

fn main() {
    let lock = RwLock::<LocalRaw, u32>::new(0);
    let guard = lock.upgradable_read();
    require_sync(&guard);
}
