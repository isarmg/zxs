//! UI tasks owned by this document's root, not by the current render effect.
//!
//! Closing the root cancels pending UI futures before they can touch disposed
//! signals. It does not establish whether an already issued server operation
//! committed, abort a transport, or retry an operation. An already requested
//! server logout deliberately uses a detached task to finish revocation.
use futures::future::{AbortHandle, Abortable};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    future::Future,
    rc::{Rc, Weak},
};

thread_local! {
    static CURRENT: RefCell<Weak<RefCell<Inner>>> = const { RefCell::new(Weak::new()) };
}

#[derive(Default)]
struct Inner {
    closed: bool,
    next: u64,
    pending: BTreeMap<u64, AbortHandle>,
}

#[derive(Clone, Default)]
struct TaskScope(Rc<RefCell<Inner>>);

impl TaskScope {
    #[cfg(any(target_arch = "wasm32", test))]
    fn close(&self) {
        let pending = {
            let mut inner = self.0.borrow_mut();
            inner.closed = true;
            std::mem::take(&mut inner.pending)
        };
        // Waking/dropping tasks must never happen under the registry borrow.
        for (_, handle) in pending {
            handle.abort();
        }
    }

    fn wrap<F: Future<Output = ()>>(&self, future: F) -> Option<impl Future<Output = ()> + use<F>> {
        let (handle, registration) = AbortHandle::new_pair();
        let id = {
            let mut inner = self.0.borrow_mut();
            if inner.closed {
                return None;
            }
            let id = inner.next;
            inner.next += 1;
            inner.pending.insert(id, handle);
            id
        };
        let guard = Registration {
            scope: Rc::downgrade(&self.0),
            id,
        };
        Some(async move {
            let _guard = guard;
            match Abortable::new(future, registration).await {
                Ok(()) | Err(futures::future::Aborted) => {}
            }
        })
    }
}

struct Registration {
    scope: Weak<RefCell<Inner>>,
    id: u64,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(scope) = self.scope.upgrade() {
            scope.borrow_mut().pending.remove(&self.id);
        }
    }
}

/// Called once inside the application root owner. The runtime's single-document
/// lifecycle prevents replacing this scope or attaching old callbacks to a new root.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn install() {
    let scope = TaskScope::default();
    CURRENT.with(|current| *current.borrow_mut() = Rc::downgrade(&scope.0));
    let scope = send_wrapper::SendWrapper::new(scope);
    crate::prelude::on_cleanup(move || scope.take().close());
}

/// Schedule a UI continuation for the mounted document, including from event
/// callbacks with no current reactive owner. After teardown (or before mounting)
/// the future is dropped without polling. This does not promise network rollback.
pub fn spawn_local(future: impl Future<Output = ()> + 'static) {
    let scope = CURRENT
        .with(|current| current.borrow().upgrade())
        .map(TaskScope);
    if let Some(task) = scope.and_then(|scope| scope.wrap(future)) {
        js_sys::futures::spawn_local(task);
    }
}

#[cfg(test)]
#[path = "../../../test/interface/unit/tasks/tests.rs"]
mod tests;
