// Compile-only: require_send must reject the future with E0277 for its Rc payload.
// No threads are created and the future is never polled.
use futures_intrusive::sync::Mutex;
use std::{cell::Cell, rc::Rc};

fn require_send<T: Send>(_: T) {}

fn main() {
    let mutex = Mutex::new(Rc::new(Cell::new(0_i32)), true);
    require_send(mutex.lock());
}
