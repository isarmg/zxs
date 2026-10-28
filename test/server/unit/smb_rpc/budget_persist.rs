//! Test-only scheduling and counting around the real persistence functions;
//! no fake filesystem.
use super::{ApiError, Owner, Path, persistence};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Barrier};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Point {
    Before,
    After,
}
thread_local! {
    static PAUSE: RefCell<Option<(Point, Arc<Barrier>)>> = const { RefCell::new(None) };
    static CREATES: Cell<usize> = const { Cell::new(0) };
    static GROWS: Cell<usize> = const { Cell::new(0) };
}
/// Durable budget record writes attempted on this thread: (create, grow).
pub(in crate::smb_rpc) fn record_writes() -> (usize, usize) {
    (CREATES.get(), GROWS.get())
}
pub(super) fn pause_at(point: Point, barrier: Arc<Barrier>) {
    PAUSE.with(|slot| *slot.borrow_mut() = Some((point, barrier)));
}
fn pause(point: Point) {
    let barrier = PAUSE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|(at, _)| *at == point) {
            slot.take().map(|(_, barrier)| barrier)
        } else {
            None
        }
    });
    if let Some(barrier) = barrier {
        barrier.wait();
        barrier.wait();
    }
}
pub(super) fn create(root: &Path, source: &Path, owner: &Owner) -> Result<(), ApiError> {
    CREATES.set(CREATES.get() + 1);
    pause(Point::Before);
    let result = persistence::create(root, source, owner);
    pause(Point::After);
    result
}
pub(super) fn grow(root: &Path, source: &Path, owner: &Owner) -> Result<(), ApiError> {
    GROWS.set(GROWS.get() + 1);
    pause(Point::Before);
    let result = persistence::grow(root, source, owner);
    pause(Point::After);
    result
}
