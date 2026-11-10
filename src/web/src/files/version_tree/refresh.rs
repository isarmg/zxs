//! One outstanding measurement frame per mounted tree. Observer registration
//! remains immediate so newly inserted nodes cannot miss resize notifications.
use std::{cell::Cell, rc::Rc};
use wasm_bindgen::{JsCast, closure::Closure};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum FrameState {
    #[default]
    Idle,
    Scheduled(i32),
    Closed,
}

impl FrameState {
    fn begin(&mut self) -> bool {
        if matches!(self, Self::Scheduled(_)) {
            *self = Self::Idle;
            true
        } else {
            false
        }
    }

    fn close(&mut self) -> Option<i32> {
        match std::mem::replace(self, Self::Closed) {
            Self::Scheduled(id) => Some(id),
            Self::Idle | Self::Closed => None,
        }
    }
}

pub(super) struct FrameRefresh {
    window: web_sys::Window,
    state: Cell<FrameState>,
    update: Box<dyn Fn()>,
}

impl FrameRefresh {
    pub(super) fn new(window: web_sys::Window, update: impl Fn() + 'static) -> Rc<Self> {
        Rc::new(Self {
            window,
            state: Cell::new(FrameState::Idle),
            update: Box::new(update),
        })
    }

    pub(super) fn request(self: &Rc<Self>) {
        if self.state.get() != FrameState::Idle {
            return;
        }
        let weak = Rc::downgrade(self);
        // JS owns this callback, as in dom_helpers' frame helper. Cancellation
        // can therefore release it through host GC without leaving an invalid
        // dropped Rust closure registered with the browser. It owns no signals.
        let callback = Closure::<dyn FnMut(f64)>::new(move |_: f64| {
            if let Some(refresh) = weak.upgrade() {
                refresh.run();
            }
        })
        .into_js_value();
        match self
            .window
            .request_animation_frame(callback.unchecked_ref())
        {
            Ok(id) => self.state.set(FrameState::Scheduled(id)),
            Err(error) => {
                web_sys::console::error_2(
                    &"Could not schedule version-tree measurement".into(),
                    &error,
                );
                // Scheduling failed at the host boundary. Do not substitute an
                // immediate layout read; remain Idle for a later notification.
            }
        }
    }

    fn run(&self) {
        let mut state = self.state.get();
        let ready = state.begin();
        self.state.set(state);
        if ready {
            (self.update)();
        }
    }

    pub(super) fn close(&self) {
        let mut state = self.state.get();
        let pending = state.close();
        self.state.set(state);
        if let Some(id) = pending
            && let Err(error) = self.window.cancel_animation_frame(id)
        {
            // A late callback sees Closed (or a dead Weak) and cannot read a
            // cleaned-up owner, even when the host rejects cancellation.
            web_sys::console::warn_2(&"Could not cancel version-tree measurement".into(), &error);
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/version_tree/refresh_tests.rs"]
mod tests;
