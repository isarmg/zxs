//! CSR mounting and reactive ownership. This product has no hydration mode.
use crate::IntoView;
use reactive_state::owner::Owner;
use task_dispatch::Executor;
use view_renderer::{
    dom::body,
    view::{Mountable, Render},
};
use web_sys::HtmlElement;

/// Build and mount a browser CSR application to the document body.
pub fn mount_to_body<F, N>(app: F)
where
    F: FnOnce() -> N + 'static,
    N: IntoView,
{
    mount_to(body(), app).forget();
}

/// Mount one CSR tree and retain its reactive owner until explicitly dropped.
pub fn mount_to<F, N>(parent: HtmlElement, app: F) -> UnmountHandle<N::State>
where
    F: FnOnce() -> N + 'static,
    N: IntoView,
{
    match Executor::init_wasm_bindgen() {
        Ok(()) | Err(task_dispatch::ExecutorError::AlreadySet) => {}
    }
    let owner = Owner::new();
    let mountable = owner.with(move || {
        let mut view = app().into_view().build();
        view.mount(&parent, None);
        view
    });
    UnmountHandle { owner, mountable }
}

/// Build and mount a CSR view into the engine's DOM renderer.
pub fn mount_to_renderer<F, N>(
    parent: &view_renderer::renderer::types::Element,
    app: F,
) -> UnmountHandle<N::State>
where
    F: FnOnce() -> N + 'static,
    N: Render,
{
    match Executor::init_wasm_bindgen() {
        Ok(()) | Err(task_dispatch::ExecutorError::AlreadySet) => {}
    }
    let owner = Owner::new();
    let mountable = owner.with(move || {
        let mut view = app().build();
        view.mount(parent, None);
        view
    });
    UnmountHandle { owner, mountable }
}

/// Dropping the handle unmounts its DOM tree and then releases scoped resources.
#[must_use = "Keep this handle alive, or call forget for an application root"]
pub struct UnmountHandle<M: Mountable> {
    owner: Owner,
    mountable: M,
}
impl<M: Mountable> UnmountHandle<M> {
    /// Keep an application root mounted until its document is destroyed.
    pub fn forget(self) {
        std::mem::forget(self);
    }
}
impl<M: Mountable> Drop for UnmountHandle<M> {
    fn drop(&mut self) {
        self.mountable.unmount();
        // A view, context, or external caller may retain an Owner clone.
        // Unmount is a disposal boundary, not merely an Arc decrement.
        self.owner.cleanup();
    }
}

#[cfg(test)]
#[path = "../../../test/interface/unit/mount/tests.rs"]
mod tests;
