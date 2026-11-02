// Baseline accepts this; the candidate must reject sharing a Cell payload.
use std::cell::Cell;
use std::sync::Arc;
use synchronization::ReentrantMutex;

fn require_sync<T: ?Sized + Sync>(_: &T) {}

fn main() {
    let lock = Arc::new(ReentrantMutex::new(Cell::new(0_u32)));
    let guard = lock.lock_arc();
    require_sync(&guard);
}
