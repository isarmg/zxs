//! Restricted, non-default fault-injection seam at the actual authorization boundary.
thread_local! {
    static AFTER_RESTORE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub(in crate::setup_recovery) fn after_next_restore(action: impl FnOnce() + 'static) {
    AFTER_RESTORE.with(|slot| *slot.borrow_mut() = Some(Box::new(action)));
}

pub(super) fn after_restore() {
    let action = AFTER_RESTORE.with(|slot| slot.borrow_mut().take());
    if let Some(action) = action { action(); }
}

pub(in crate::setup_recovery) fn pending() -> bool {
    AFTER_RESTORE.with(|slot| slot.borrow().is_some())
}
