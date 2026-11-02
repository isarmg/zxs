// Both revisions must reject this with send_guard disabled and enabled.
use std::sync::Arc;
use synchronization::ReentrantMutex;

fn require_send<T: Send>(_: T) {}

fn main() {
    let lock = Arc::new(ReentrantMutex::new(1_i32));
    let guard = lock.lock_arc();
    require_send(guard);
}
