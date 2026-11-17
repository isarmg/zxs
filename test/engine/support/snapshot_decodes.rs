//! Observe the existing decode seam without changing snapshot admission.
use crate::journal::Snapshot;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;

thread_local! {
    pub(crate) static SNAPSHOT_LOAD_COUNT: Cell<usize> = const { Cell::new(0) };
    static WATCHED: RefCell<Option<Counter>> = const { RefCell::new(None) };
}

struct Counter {
    hash: String,
    count: usize,
}

/// One test-thread observation of one complete canonical snapshot descriptor.
/// No State, authority or decoded result is retained or reused.
pub(crate) struct Watch {
    hash: String,
    _thread: PhantomData<Rc<()>>,
}

impl Watch {
    pub(crate) fn new(hash: &str) -> Self {
        WATCHED.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none(), "snapshot decode observations may not overlap");
            *slot = Some(Counter { hash: hash.to_owned(), count: 0 });
        });
        Self { hash: hash.to_owned(), _thread: PhantomData }
    }

    pub(crate) fn count(&self) -> usize {
        WATCHED.with(|slot| {
            let slot = slot.borrow();
            let counter = slot.as_ref().expect("live snapshot decode observation");
            assert_eq!(counter.hash, self.hash);
            counter.count
        })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        WATCHED.with(|slot| {
            *slot.borrow_mut() = None;
        });
    }
}

pub(crate) fn record(snapshot: &Snapshot) {
    // Preserve the existing total counter and its precise pre-validation point.
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(count.get() + 1));
    WATCHED.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(counter) = slot.as_mut() else { return };
        let hash = crate::object_hash("snapshot", snapshot)
            .expect("test snapshot descriptor has a canonical object hash");
        if hash == counter.hash {
            counter.count += 1;
        }
    });
}
