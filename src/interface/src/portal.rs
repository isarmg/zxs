use crate::{IntoView, children::TypedChildrenFn, mount};
use dom_helpers::helpers::document;
use reactive_state::{effect::Effect, graph::untrack, owner::Owner};
use std::sync::Arc;
use view_macros::component;

/// Renders components somewhere else in the DOM.
///
/// Useful for inserting modals and tooltips outside of a cropping layout.
/// If no mount point is given, the portal is inserted in `document.body`;
/// it is wrapped in a `<div>` unless  `is_svg` is `true` in which case it's wrapped in a `<g>`.
/// This CSR interface mounts into the regular DOM. The compatibility
/// `use_shadow` argument must remain `false`; `true` panics before mounting.
#[component]
pub fn Portal<V>(
    /// Target element where the children will be appended
    #[prop(into, optional)]
    mount: Option<web_sys::Element>,
    /// Retained for compatibility. Must be `false`; shadow roots are unsupported.
    #[prop(optional)]
    use_shadow: bool,
    /// When using SVG this has to be set to `true`. Defaults to `false`.
    #[prop(optional)]
    is_svg: bool,
    /// The children to teleport into the `mount` element
    children: TypedChildrenFn<V>,
) -> impl IntoView
where
    V: IntoView + 'static,
{
    assert!(
        !use_shadow,
        "Portal use_shadow=true is unsupported by this CSR interface"
    );

    if cfg!(target_arch = "wasm32") {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::JsCast;

        let mount =
            mount.unwrap_or_else(|| document().body().expect("body to exist").unchecked_into());
        let children = children.into_inner();

        Effect::new(move |_| {
            let container = if is_svg {
                document()
                    .create_element_ns(Some("http://www.w3.org/2000/svg"), "g")
                    .expect("SVG element creation to work")
            } else {
                document()
                    .create_element("div")
                    .expect("HTML element creation to work")
            };

            let render_root = container.clone();

            if let Err(error) = mount.append_child(&container) {
                web_sys::console::error_2(&"Could not attach the portal".into(), &error);
                return;
            }
            let handle = SendWrapper::new((
                mount::mount_to(render_root.unchecked_into(), {
                    let children = Arc::clone(&children);
                    move || untrack(|| children())
                }),
                mount.clone(),
                container,
            ));

            Owner::on_cleanup({
                move || {
                    let (handle, mount, container) = handle.take();
                    drop(handle);
                    if let Err(error) = mount.remove_child(&container) {
                        web_sys::console::warn_2(&"Could not detach the portal".into(), &error);
                    }
                }
            })
        });
    }
}
