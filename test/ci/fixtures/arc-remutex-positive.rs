// Public Arc reentrant guard controls; main runs every case to completion.
use std::cell::Cell;
use std::sync::Arc;
use synchronization::api::ArcReentrantMutexGuard;
use synchronization::{RawMutex, RawThreadId, ReentrantMutex};

fn require_sync<T: ?Sized + Sync>(_: &T) {}
fn require_sized<T: Sized>() {}

fn i32_shared() {
    let lock = Arc::new(ReentrantMutex::new(42_i32));
    let guard = lock.lock_arc();
    require_sync(&guard);
    std::thread::scope(|scope| {
        let shared = &guard;
        scope.spawn(move || assert_eq!(**shared, 42));
    });
    drop(guard);
    assert_eq!(*lock.lock_arc(), 42);
}

fn cell_local() {
    let lock = Arc::new(ReentrantMutex::new(Cell::new(1_u32)));
    let first = lock.lock_arc();
    first.set(7);
    let second = lock.lock_arc();
    assert_eq!(second.get(), 7);
    drop(second);
    assert_eq!(first.get(), 7);
    drop(first);
    assert_eq!(lock.lock().get(), 7);
}

fn borrowed<'a>(text: &'a str) {
    let lock = Arc::new(ReentrantMutex::new(text));
    let guard = lock.lock_arc();
    require_sync(&guard);
    assert_eq!(*guard, text);
    drop(guard);
    assert_eq!(*lock.lock(), text);
    require_sized::<ArcReentrantMutexGuard<RawMutex, RawThreadId, dyn AsRef<str> + Send + Sync + 'a>>(
    );
}

fn unsized_payload_controls() {
    require_sized::<ArcReentrantMutexGuard<RawMutex, RawThreadId, str>>();
    require_sized::<ArcReentrantMutexGuard<RawMutex, RawThreadId, [u8]>>();
    let lock: Arc<synchronization::api::ReentrantMutex<RawMutex, RawThreadId, [u8]>> =
        Arc::new(synchronization::api::ReentrantMutex::new([1_u8, 2, 3]));
    let guard = lock.lock_arc();
    require_sync(&guard);
    assert_eq!(&*guard, &[1_u8, 2, 3]);
}

struct BorrowedDrop<'a>(&'a Cell<u32>);

impl Drop for BorrowedDrop<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[expect(
    clippy::arc_with_non_send_sync,
    reason = "The Arc guard API must accept a local payload borrowing a non-Sync drop counter."
)]
fn borrowed_drop() {
    let drops = Cell::new(0);
    let lock = Arc::new(ReentrantMutex::new(BorrowedDrop(&drops)));
    let guard = lock.lock_arc();
    drop(guard);
    assert_eq!(drops.get(), 0);
    drop(lock);
    assert_eq!(drops.get(), 1);

    let lock = Arc::new(ReentrantMutex::new(BorrowedDrop(&drops)));
    let guard = lock.lock_arc();
    drop(lock);
    assert_eq!(drops.get(), 1);
    drop(guard);
    assert_eq!(drops.get(), 2);
}

fn main() {
    i32_shared();
    cell_local();
    let text = String::from("nonstatic data");
    borrowed(&text);
    unsized_payload_controls();
    borrowed_drop();
    println!("ARC_REMUTEX_POSITIVE_CONTROLS_COMPLETE");
}
