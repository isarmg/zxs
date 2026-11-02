// Pinning the movable guard handle must not require its payload to be Unpin.
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::Arc;
use synchronization::ReentrantMutex;

struct PinnedData {
    value: i32,
    _pin: PhantomPinned,
}

fn require_unpin<T: ?Sized + Unpin>(_: &T) {}

fn main() {
    let lock = Arc::new(ReentrantMutex::new(PinnedData {
        value: 9,
        _pin: PhantomPinned,
    }));
    let mut guard = lock.lock_arc();
    require_unpin(&guard);
    let address = &*guard as *const PinnedData;
    let movable = Pin::into_inner(Pin::new(&mut guard));
    require_unpin(movable);
    assert_eq!(address, &**movable as *const PinnedData);
    assert_eq!(movable.value, 9);
    println!("ARC_REMUTEX_PHANTOM_PINNED_UNPIN_COMPLETE");
}
