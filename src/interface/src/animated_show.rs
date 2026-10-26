use crate::{IntoView, children::ChildrenFn, component, control_flow::Show};
use core::time::Duration;
use dom_helpers::helpers::TimeoutHandle;
use reactive_state::{
    effect::RenderEffect,
    owner::{StoredValue, on_cleanup},
    signal::RwSignal,
    traits::{Get, GetUntracked, GetValue, Set, SetValue},
    wrappers::read::Signal,
};
use view_macros::view;
use view_renderer::prelude::*;

/// A component that will show its children when the `when` condition is `true`.
/// Additionally, you need to specify a `hide_delay`. If the `when` condition changes to `false`,
/// the unmounting of the children will be delayed by the specified Duration.
/// If you provide the optional `show_class` and `hide_class`, you can create very easy mount /
/// unmount animations.
///
/// ```rust
/// # use core::time::Duration;
/// # use web_interface::prelude::*;
/// # #[component]
/// # pub fn App() -> impl IntoView {
/// let show = RwSignal::new(false);
///
/// view! {
///     <div
///         class="hover-me"
///         on:mouseenter=move |_| show.set(true)
///         on:mouseleave=move |_| show.set(false)
///     >
///         "Hover Me"
///     </div>
///
///     <AnimatedShow
///        when=show
///        show_class="fade-in-1000"
///        hide_class="fade-out-1000"
///        hide_delay=Duration::from_millis(1000)
///     >
///        <div class="here-i-am">
///            "Here I Am!"
///        </div>
///     </AnimatedShow>
/// }
/// # }
/// ```
#[component]
pub fn AnimatedShow(
    /// The components Show wraps
    children: ChildrenFn,
    /// If the component should show or not
    #[prop(into)]
    when: Signal<bool>,
    /// Optional CSS class to apply if `when == true`
    #[prop(optional)]
    show_class: &'static str,
    /// Optional CSS class to apply if `when == false`
    #[prop(optional)]
    hide_class: &'static str,
    /// The timeout after which the component will be unmounted if `when == false`
    hide_delay: Duration,
) -> impl IntoView {
    let handle: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let cls = RwSignal::new(if when.get_untracked() {
        show_class
    } else {
        hide_class
    });
    let show = RwSignal::new(when.get_untracked());

    let eff = RenderEffect::new(move |_| {
        // A derived condition can notify repeatedly with false. Replace its
        // pending hide timer before either branch, so an older callback cannot
        // hide children after a later true transition or outlive cleanup.
        if let Some(h) = handle.get_value() {
            h.clear();
        }
        if when.get() {
            cls.set(show_class);
            show.set(true);
        } else {
            cls.set(hide_class);

            match dom_helpers::helpers::set_timeout_with_handle(move || show.set(false), hide_delay)
            {
                Ok(h) => handle.set_value(Some(h)),
                Err(error) => {
                    web_sys::console::warn_2(&"Could not schedule delayed hiding".into(), &error);
                    handle.set_value(None);
                    show.set(false);
                }
            }
        }
    });

    on_cleanup(move || {
        if let Some(Some(h)) = handle.try_get_value() {
            h.clear();
        }
        drop(eff);
    });

    view! {
        <Show when=move || show.get() fallback=|| ()>
            <div class=move || cls.get()>{children()}</div>
        </Show>
    }
}
