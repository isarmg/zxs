//! ZXS browser interface: CSR primitives, presentation and application lifetime.
//!
//! The adopted CSR engine and project lifecycle compile together in this crate.
//! Original source identities and notices are retained in the repository's
//! centralized `compliance/` records.
extern crate self as web_interface;

/// Exports all the core types of the library.
pub mod prelude {
    // Traits
    // These should always be exported from the prelude
    pub use reactive_state::prelude::*;
    pub use view_renderer::prelude::*;

    // Collect the public types of the integrated CSR interface.
    mod export_types {
        pub use crate::{
            callback::*, children::*, component::*, control_flow::*, error::*, into_view::*,
            mount::*, text_prop::*,
        };
        pub use dom_helpers::helpers::*;
        pub use reactive_state::{
            actions::*,
            computed::*,
            effect::*,
            graph::untrack,
            owner::*,
            signal::*,
            wrappers::{read::*, write::*},
        };
        pub use shared_value::*;
        pub use view_macros::*;
        pub use view_renderer::{
            reactive_state::{Suspend, bind::BindAttribute, node_ref::*},
            view::{fragment::Fragment, template::ViewTemplate},
        };
    }
    pub use export_types::*;
}

/// A standard way to wrap functions and closures to pass them to components.
pub use reactive_state::callback;

/// Types that can be passed as the `children` prop of a component.
pub mod children;

/// Wrapper for intercepting component attributes.
pub mod attribute_interceptor;

#[doc(hidden)]
/// Traits used to implement component constructors.
pub mod component;

/// Tools for handling errors.
pub mod error {
    pub use throw_error::*;
}

/// CSR control-flow components such as `<Show>`, `<For>`, and `<ShowLet>`.
pub mod control_flow {
    pub use crate::{animated_show::*, for_loop::*, for_snapshot::*, show::*, show_let::*};
}
mod animated_show;

mod for_loop;
mod for_snapshot;
mod show;
mod show_let;

/// A component that allows rendering a component somewhere else.
pub mod portal;

/// Types for reactive string properties for components.
pub mod text_prop;

#[doc(hidden)]
pub use typed_builder;
pub use view_macros::*;
mod into_view;
#[doc(inline)]
pub use dom_helpers;
pub use into_view::IntoView;
mod provider;
#[doc(inline)]
pub use view_renderer;
/// Mount a browser CSR tree into the DOM and retain its reactive owner.
pub mod mount;
#[doc(inline)]
pub use reactive_state as reactive;
#[doc(inline)]
pub use shared_value;
#[doc(inline)]
pub use view_alternatives as either;

/// Provide and access data along the reactive graph, sharing data without directly passing arguments.
pub mod context {
    pub use crate::provider::*;
    pub use reactive_state::owner::{provide_context, use_context};
}

/// HTML attribute types.
#[doc(inline)]
pub use view_renderer::html::attribute as attr;
/// HTML element types.
#[doc(inline)]
pub use view_renderer::html::element as html;
/// HTML event types.
#[doc(no_inline)]
pub use view_renderer::html::event as ev;
/// SVG element types.
#[doc(inline)]
pub use view_renderer::svg;

/// Utilities for simple isomorphic logging to the console or terminal.
pub mod logging {
    pub use dom_helpers::{debug_error, debug_log, debug_warn, error, log, warn};
}

/// Utilities for working with asynchronous tasks.
pub mod task {
    use reactive_state::computed::ScopedFuture;
    use std::future::Future;
    use task_dispatch::Executor;

    /// Spawns a thread-safe [`Future`].
    ///
    /// This will be run with the current reactive owner and observer using a [`ScopedFuture`].
    #[track_caller]
    #[inline(always)]
    pub fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        let fut = ScopedFuture::new(fut);

        #[cfg(not(target_family = "wasm"))]
        Executor::spawn(fut);

        #[cfg(target_family = "wasm")]
        Executor::spawn_local(fut);
    }

    /// Spawns a [`Future`] that cannot be sent across threads.
    #[track_caller]
    #[inline(always)]
    pub fn spawn_local(fut: impl Future<Output = ()> + 'static) {
        Executor::spawn_local(fut)
    }

    /// Waits until the next "tick" of the current async executor.
    pub async fn tick() {
        Executor::tick().await
    }

    pub use reactive_state::{spawn_local_scoped, spawn_local_scoped_with_cancellation};
}

#[cfg(target_arch = "wasm32")]
mod browser_support;
pub mod components;
#[expect(
    clippy::doc_lazy_continuation,
    reason = "Preserve the audited G4 source snapshot byte-for-byte; its legacy math documentation uses non-indented continuation lines"
)]
pub mod contours;
pub mod i18n;
#[cfg(any(target_arch = "wasm32", test))]
mod lifecycle;
pub mod startup;
pub mod tasks;
pub mod theme;

use wasm_bindgen::JsValue;

/// Begin CSR startup. Optional UI fonts load independently of functional readiness.
/// Errors before scheduling are returned; later failures publish the startup failure event.
pub fn mount<F, V>(root_id: &str, app: F) -> Result<(), JsValue>
where
    F: FnOnce() -> V + 'static,
    V: IntoView + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        browser::mount(root_id, app)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (root_id, app);
        Err(JsValue::NULL)
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use crate::prelude::IntoAny;
    use futures::{FutureExt, channel::oneshot};
    use gloo_events::EventListener;
    use js_sys::futures::JsFuture;
    use std::{cell::RefCell, rc::Rc};
    use wasm_bindgen::JsCast;

    thread_local! {
        static APPLICATION: RefCell<lifecycle::Lifecycle<Box<Application>>> =
            const { RefCell::new(lifecycle::Lifecycle::new()) };
    }
    struct Application {
        // Keep the owner and controller until an uncached page leaves.
        view: mount::UnmountHandle<view_renderer::view::any_view::AnyViewState>,
        contours: contours::ContourController,
        _listeners: (EventListener, EventListener),
    }
    impl Drop for Application {
        fn drop(&mut self) {
            self.contours.stop();
            theme::cleanup();
            // `view` then unmounts the tree and releases its reactive owner.
            let _ = &self.view;
        }
    }
    fn cleanup() {
        let application = APPLICATION.with(|slot| slot.borrow_mut().close());
        drop(application);
    }
    pub fn mount<F, V>(root_id: &str, app: F) -> Result<(), JsValue>
    where
        F: FnOnce() -> V + 'static,
        V: IntoView + 'static,
    {
        console_error_panic_hook::set_once();
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| JsValue::from_str("Browser document is unavailable"))?;
        let root = document
            .get_element_by_id(root_id)
            .ok_or_else(|| JsValue::from_str("Application root is missing"))?
            .dyn_into::<web_sys::HtmlElement>()?;
        let window = document
            .default_view()
            .ok_or_else(|| JsValue::from_str("Browser window is unavailable"))?;
        if !APPLICATION.with(|slot| slot.borrow_mut().begin()) {
            return Err(JsValue::from_str("Application mounting already began"));
        }
        if let Err(error) =
            startup::begin(&document).and_then(|()| browser_support::require(&window))
        {
            cleanup();
            startup::failed(&document, &error);
            return Err(error);
        }
        // Register before yielding, and retain these same listeners through the
        // mounted lifetime. Background font work must not mutate a disposed document.
        let (cancel, cancelled) = oneshot::channel::<()>();
        let cancel = Rc::new(RefCell::new(Some(cancel)));
        let failed_cancel = cancel.clone();
        let failed = EventListener::new(&document, startup::FAILED_EVENT, move |_| {
            if let Some(sender) = failed_cancel.borrow_mut().take() {
                match sender.send(()) {
                    Ok(()) | Err(()) => {}
                }
            }
            cleanup();
        });
        let pagehide = EventListener::new(&window, "pagehide", move |event| {
            if !event
                .dyn_ref::<web_sys::PageTransitionEvent>()
                .is_some_and(|event| event.persisted())
            {
                if let Some(sender) = cancel.borrow_mut().take() {
                    match sender.send(()) {
                        Ok(()) | Err(()) => {}
                    }
                }
                cleanup();
            }
        });
        i18n::initialize();
        theme::initialize();
        js_sys::futures::spawn_local(async move {
            let result: Result<(), JsValue> = async {
                ensure_starting()?;
                startup::ensure_pending(&document)?;
                root.set_text_content(None);
                let view = mount::mount_to(root, move || {
                    tasks::install();
                    app().into_any()
                });
                // Components may synchronously dispatch lifecycle events too.
                ensure_starting()?;
                let html = document
                    .document_element()
                    .ok_or_else(|| JsValue::from_str("Browser document root is missing"))?;
                html.set_attribute("data-zxs-appearance", "content-blocks")?;
                html.set_attribute("data-zxs-selection", "underline")?;
                let contours = contours::start(&document)?;
                let application = Box::new(Application {
                    view,
                    contours,
                    _listeners: (pagehide, failed),
                });
                // Publish before dispatch: a synchronous ready listener can
                // navigate away or reject startup and must find the live owner.
                let installed = APPLICATION.with(|slot| slot.borrow_mut().install(application));
                if let Err(application) = installed {
                    drop(application);
                    return Err(JsValue::from_str("Application startup was cancelled"));
                }
                startup::ready(&document)?;
                if !APPLICATION.with(|slot| slot.borrow().is_mounted()) {
                    return Err(JsValue::from_str("Application startup was cancelled"));
                }
                // Font download/decoding is enhancement work, not an application
                // startup dependency. Start it only after publishing ownership.
                let font_document = document.clone();
                js_sys::futures::spawn_local(async move {
                    if let Some(result) = wait_for_fonts(&font_document, cancelled).await
                        && APPLICATION.with(|slot| slot.borrow().is_mounted())
                        && let Some(html) = font_document.document_element()
                    {
                        let state = if result.is_ok() { "ready" } else { "failed" };
                        if let Err(error) = html.set_attribute("data-zxs-fonts", state) {
                            web_sys::console::warn_2(
                                &"Could not publish font readiness".into(),
                                &error,
                            );
                        }
                    }
                });
                Ok(())
            }
            .await;
            if let Err(error) = result {
                cleanup();
                theme::cleanup();
                startup::failed(&document, &error);
            }
        });
        Ok(())
    }
    fn ensure_starting() -> Result<(), JsValue> {
        if APPLICATION.with(|slot| slot.borrow().is_starting()) {
            Ok(())
        } else {
            Err(JsValue::from_str("Application startup was cancelled"))
        }
    }
    async fn wait_for_fonts(
        document: &web_sys::Document,
        cancelled: oneshot::Receiver<()>,
    ) -> Option<Result<(), JsValue>> {
        let fonts = async {
            let fonts = document.fonts();
            let regular = fonts.load("16px \"ZxsUI\"");
            let bold = fonts.load("bold 16px \"ZxsUI\"");
            let (regular, bold) =
                futures::try_join!(JsFuture::from(regular), JsFuture::from(bold))?;
            if js_sys::Array::from(&regular).length() == 0
                || js_sys::Array::from(&bold).length() == 0
            {
                return Err(JsValue::from_str("Application font styles are missing"));
            }
            // FontFaceSet::load resolves after the requested faces are loaded.
            // Waiting for document.fonts.ready here also waits for unrelated
            // document fonts and layout, although neither is a startup resource.
            Ok(())
        }
        .fuse();
        let cancelled = cancelled.fuse();
        futures::pin_mut!(fonts, cancelled);
        futures::select! {
            result = fonts => Some(result),
            _ = cancelled => None,
        }
    }
}
