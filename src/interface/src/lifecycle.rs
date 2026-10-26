//! Transactional ownership for a single document lifetime.
//!
//! Browser events can run synchronously during mounting and ready notification.
//! Closing must therefore invalidate a pending mount, not only remove a view.
#[derive(Debug, PartialEq, Eq)]
enum State<T> {
    Idle,
    Starting,
    Mounted(T),
    Closed,
}

pub(crate) struct Lifecycle<T>(State<T>);
impl<T> Lifecycle<T> {
    pub(crate) const fn new() -> Self {
        Self(State::Idle)
    }

    pub(crate) fn begin(&mut self) -> bool {
        if matches!(self.0, State::Idle) {
            self.0 = State::Starting;
            true
        } else {
            false
        }
    }

    pub(crate) fn is_starting(&self) -> bool {
        matches!(self.0, State::Starting)
    }

    pub(crate) fn is_mounted(&self) -> bool {
        matches!(self.0, State::Mounted(_))
    }

    pub(crate) fn install(&mut self, value: T) -> Result<(), T> {
        if self.is_starting() {
            self.0 = State::Mounted(value);
            Ok(())
        } else {
            Err(value)
        }
    }

    // Return resources to the caller: destructors may reenter the registry and
    // must never run while its RefCell is borrowed.
    pub(crate) fn close(&mut self) -> Option<T> {
        match std::mem::replace(&mut self.0, State::Closed) {
            State::Mounted(value) => Some(value),
            State::Idle | State::Starting | State::Closed => None,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/interface/unit/lifecycle/tests.rs"]
mod tests;
