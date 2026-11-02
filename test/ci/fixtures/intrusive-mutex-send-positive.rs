// Compile-only controls for the public mutex API. No threads or future polling.
use futures_intrusive::sync::Mutex;
use std::cell::Cell;

fn require_send<T: Send>(_: T) {}
fn require_sync<T: Sync>(_: &T) {}

fn borrowed_payload_future<T: Send>(mutex: &Mutex<T>) {
    require_send(mutex.lock());
}

fn main() {
    let mutex = Mutex::new(1_i32, true);
    borrowed_payload_future(&mutex);
    require_send(mutex.lock());

    let guard = mutex
        .try_lock()
        .expect("invariant: fresh mutex is unlocked");
    require_sync(&guard);
    require_send(guard);

    let cell_mutex = Mutex::new(Cell::new(2_i32), false);
    require_send(cell_mutex.lock());
}
