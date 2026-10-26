//! Document-visible background reads. Pausing never cancels an admitted request
//! or discards its receipt: session/operation ownership still decides settlement.
use crate::ui::State;
use web_interface::prelude::*;

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Default)]
struct Visibility {
    running: bool,
}
#[cfg(any(target_arch = "wasm32", test))]
impl Visibility {
    /// Return a transition only once, including repeated pageshow/visibility events.
    fn change(&mut self, visible: bool) -> Option<bool> {
        if self.running == visible {
            return None;
        }
        self.running = visible;
        Some(visible)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Demand {
    operations: bool,
    management: bool,
    telemetry: bool,
}
fn demand(state: &State, visible: bool) -> Demand {
    Demand {
        operations: visible && state.session_active(),
        // Coarse management health is public and gates writes on every page.
        management: visible
            && !state.logout_pending()
            && !state.logout_unconfirmed()
            && !state.restoring(),
        telemetry: visible && state.session_active() && state.page == crate::ui::Page::Status,
    }
}

pub(crate) fn install(state: RwSignal<State>) {
    let operations = move |visible| {
        if state.try_with_untracked(|state| demand(state, visible).operations) == Some(true) {
            crate::files::poll(state);
        }
    };
    let health = move |visible| {
        let demand = state
            .try_with_untracked(|state| demand(state, visible))
            .unwrap_or_default();
        if demand.management {
            crate::management::load(state);
        }
        if demand.telemetry {
            crate::monitor::load(state);
        }
    };
    #[cfg(target_arch = "wasm32")]
    browser::install(operations, health);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (operations, health);
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use gloo_timers::callback::Interval;
    use std::{cell::RefCell, rc::Rc};
    use wasm_bindgen::{JsCast, closure::Closure};
    use web_sys::{Document, Event, EventTarget};

    struct Reads {
        document: Document,
        operations: Box<dyn Fn(bool)>,
        health: Box<dyn Fn(bool)>,
        visibility: RefCell<Visibility>,
        timers: RefCell<Option<(Interval, Interval)>>,
    }
    impl Reads {
        fn operations(&self) {
            (self.operations)(!self.document.hidden());
        }
        fn health(&self) {
            (self.health)(!self.document.hidden());
        }
        fn visibility(self: &Rc<Self>, visible: bool, refresh: bool) {
            let change = self.visibility.borrow_mut().change(visible);
            match change {
                Some(true) => {
                    let weak = Rc::downgrade(self);
                    let operations = Interval::new(2_000, move || {
                        if let Some(reads) = weak.upgrade() {
                            reads.operations();
                        }
                    });
                    let weak = Rc::downgrade(self);
                    let health = Interval::new(5_000, move || {
                        if let Some(reads) = weak.upgrade() {
                            reads.health();
                        }
                    });
                    *self.timers.borrow_mut() = Some((operations, health));
                    if refresh {
                        self.operations();
                        self.health();
                    }
                }
                Some(false) => {
                    self.timers.borrow_mut().take();
                }
                None => {}
            }
        }
    }

    struct Listener {
        target: EventTarget,
        name: &'static str,
        callback: Closure<dyn FnMut(Event)>,
    }
    impl Drop for Listener {
        fn drop(&mut self) {
            if let Err(error) = self.target.remove_event_listener_with_callback(
                self.name,
                self.callback.as_ref().unchecked_ref(),
            ) {
                web_sys::console::warn_2(
                    &"Could not remove the background-read listener".into(),
                    &error,
                );
            }
        }
    }

    pub(super) fn install(operations: impl Fn(bool) + 'static, health: impl Fn(bool) + 'static) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let reads = Rc::new(Reads {
            document: document.clone(),
            operations: Box::new(operations),
            health: Box::new(health),
            visibility: RefCell::new(Visibility::default()),
            timers: RefCell::new(None),
        });
        let mut listeners = Vec::new();
        for (target, name) in [
            (document.as_ref(), "visibilitychange"),
            (window.as_ref(), "pagehide"),
            (window.as_ref(), "pageshow"),
        ] {
            let weak = Rc::downgrade(&reads);
            let callback = Closure::wrap(Box::new(move |event: Event| {
                if let Some(reads) = weak.upgrade() {
                    reads.visibility(
                        event.type_() != "pagehide" && !reads.document.hidden(),
                        true,
                    );
                }
            }) as Box<dyn FnMut(Event)>);
            let target: &EventTarget = target;
            if let Err(error) =
                target.add_event_listener_with_callback(name, callback.as_ref().unchecked_ref())
            {
                web_sys::console::error_2(&"Background-read listener failed".into(), &error);
                // Drop already-installed listeners rather than leak callbacks.
                return;
            }
            listeners.push(Listener {
                target: target.clone(),
                name,
                callback,
            });
        }
        reads.visibility(!document.hidden(), false);
        let owned = send_wrapper::SendWrapper::new((reads, listeners));
        on_cleanup(move || drop(owned));
    }
}

#[cfg(test)]
#[path = "../../../test/web/unit/polling/tests.rs"]
mod tests;
