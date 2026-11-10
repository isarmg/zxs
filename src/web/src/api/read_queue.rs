use std::{
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

const MAX_WAITING: usize = 32;

pub(super) fn managed(method: &str, path: &str) -> bool {
    if method != "GET" {
        return false;
    }
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    path == "/api/admin/disks"
        || path.starts_with("/api/admin/disks/")
        || path.starts_with("/api/admin/storage/")
        || path.starts_with("/api/setup/")
        || matches!(path, "/api/workspaces/capacity" | "/api/capabilities")
}

#[derive(Clone, Default)]
pub(super) struct Queue(Rc<RefCell<Inner>>);

#[derive(Default)]
struct Inner {
    active: bool,
    waiting: VecDeque<Waiter>,
}

struct Waiter {
    id: Rc<()>,
    waker: Option<Waker>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AdmissionError {
    Full,
    Invalidated,
}

impl Queue {
    pub(super) fn acquire(&self) -> Result<Admission, AdmissionError> {
        let id = Rc::new(());
        let mut inner = self.0.borrow_mut();
        if inner.waiting.len() == MAX_WAITING {
            return Err(AdmissionError::Full);
        }
        inner.waiting.push_back(Waiter {
            id: Rc::clone(&id),
            waker: None,
        });
        Ok(Admission {
            queue: self.clone(),
            id: Some(id),
        })
    }

    pub(super) fn invalidate(&self) {
        let waiting = std::mem::take(&mut self.0.borrow_mut().waiting);
        for waiter in waiting {
            if let Some(waker) = waiter.waker {
                waker.wake();
            }
        }
    }
}

pub(super) struct Admission {
    queue: Queue,
    id: Option<Rc<()>>,
}

impl Future for Admission {
    type Output = Result<Permit, AdmissionError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let id = this
            .id
            .as_ref()
            .expect("invariant: admission is not complete");
        let mut inner = this.queue.0.borrow_mut();
        let Some(index) = inner
            .waiting
            .iter()
            .position(|waiter| Rc::ptr_eq(&waiter.id, id))
        else {
            this.id = None;
            return Poll::Ready(Err(AdmissionError::Invalidated));
        };
        if !inner.active && index == 0 {
            inner.waiting.pop_front();
            inner.active = true;
            this.id = None;
            return Poll::Ready(Ok(Permit(this.queue.clone())));
        }
        let waiter = &mut inner.waiting[index];
        if waiter
            .waker
            .as_ref()
            .is_none_or(|waker| !waker.will_wake(context.waker()))
        {
            waiter.waker = Some(context.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        let wake = {
            let mut inner = self.queue.0.borrow_mut();
            if let Some(index) = inner
                .waiting
                .iter()
                .position(|waiter| Rc::ptr_eq(&waiter.id, &id))
            {
                inner.waiting.remove(index);
            }
            (!inner.active)
                .then(|| {
                    inner
                        .waiting
                        .front()
                        .and_then(|waiter| waiter.waker.clone())
                })
                .flatten()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

pub(super) struct Permit(Queue);

impl Drop for Permit {
    fn drop(&mut self) {
        let wake = {
            let mut inner = self.0.0.borrow_mut();
            assert!(
                inner.active,
                "invariant: only an active permit releases admission"
            );
            inner.active = false;
            inner
                .waiting
                .front()
                .and_then(|waiter| waiter.waker.clone())
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/api/read_queue_tests.rs"]
mod tests;
