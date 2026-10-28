//! Waker probes for the retained futures-intrusive integration tests.
//!
//! Project-authored replacement for the two `futures_test::task` helpers the
//! upstream tests import; that crate is not part of the local sources. The
//! observable contract is kept: a count waker records every wake through any
//! clone, and a panic waker fails the test as soon as it is woken.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Wake, Waker};

/// Wakes recorded by the waker returned with it from [`new_count_waker`].
#[derive(Clone, Debug)]
pub struct AwokenCount(Arc<Counter>);

#[derive(Debug)]
pub struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl AwokenCount {
    pub fn get(&self) -> usize {
        self.0 .0.load(Ordering::SeqCst)
    }
}

impl PartialEq<usize> for AwokenCount {
    fn eq(&self, other: &usize) -> bool {
        self.get() == *other
    }
}

pub fn new_count_waker() -> (Waker, AwokenCount) {
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    (Waker::from(Arc::clone(&counter)), AwokenCount(counter))
}

struct Panic;

impl Wake for Panic {
    fn wake(self: Arc<Self>) {
        panic!("should not be woken");
    }
}

pub fn panic_waker() -> Waker {
    Waker::from(Arc::new(Panic))
}
