use crate::{i18n::t, prelude::*};
use wasm_bindgen::JsCast;
/// Native modal semantics retain focus containment and background inertness.
#[component]
pub fn Dialog(title: String, close: Callback<()>, children: ChildrenFn) -> impl IntoView {
    let dialog = NodeRef::<web_interface::html::Dialog>::new();
    let previous = send_wrapper::SendWrapper::new(
        web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.active_element())
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()),
    );
    Effect::new(move |_| {
        if let Some(dialog) = dialog.get() {
            if let Err(error) = dialog.show_modal() {
                web_sys::console::error_2(&"Could not open the dialog".into(), &error);
                close.run(());
                return;
            }
            if let Some(element) = dialog
                .query_selector("[data-zxs-initial-focus]")
                .expect("invariant: initial-focus selector is valid")
                && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
                && let Err(error) = element.focus()
            {
                web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error);
            }
        }
    });
    on_cleanup(move || {
        if let Some(dialog) = dialog.get_untracked() {
            dialog.close();
        }
        if let Some(previous) = previous.as_ref()
            && previous.is_connected()
            && let Err(error) = previous.focus()
        {
            web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error);
        }
    });
    View::new(
        web_interface::html::dialog()
            .child((
                web_interface::html::div()
                    .child((
                        web_interface::html::h2().child(
                            #[allow(unused_braces)]
                            {
                                IntoRender::into_render({ title.clone() })
                            },
                        ),
                        web_interface::html::button()
                            .child(
                                #[allow(unused_braces)]
                                {
                                    "×"
                                },
                            )
                            .class("zxs-button")
                            .r#type("button")
                            .aria_label(
                                ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                                    t("关闭对话框", "Close dialog"),
                                ),
                            )
                            .on(web_interface::ev::click, move |_| close.run(())),
                    ))
                    .class("zxs-dialog-heading zxs-flow"),
                IntoRender::into_render(children()),
            ))
            .class("zxs-dialog")
            .node_ref(dialog)
            .aria_label(
                ::web_interface::prelude::IntoAttributeValue::into_attribute_value(title.clone()),
            )
            .tabindex("-1")
            .on(
                web_interface::ev::Custom::new("cancel"),
                move |event: web_sys::Event| {
                    event.prevent_default();
                    close.run(());
                },
            )
            .on(web_interface::ev::keydown, move |event| {
                if event.key() != "Tab" {
                    return;
                }
                let Some(dialog) = dialog.get_untracked() else {
                    return;
                };
                if dialog
                    .query_selector("video[controls],audio[controls]")
                    .expect("invariant: media selector is valid")
                    .is_some()
                {
                    return;
                }
                let nodes = dialog
                    .query_selector_all("button,input,select,textarea,a[href],[tabindex]")
                    .expect("invariant: focusable selector is valid");
                let focusable: Vec<_> = (0..nodes.length())
                    .filter_map(|i| nodes.item(i))
                    .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
                    .filter(|e| {
                        e.tab_index() >= 0
                            && !e
                                .matches(":disabled,[hidden]")
                                .expect("invariant: disabled selector is valid")
                            && e.get_client_rects().length() > 0
                    })
                    .collect();
                let active = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.active_element());
                if let (Some(first), Some(last)) = (focusable.first(), focusable.last()) {
                    if event.shift_key()
                        && (active.as_ref() == Some(first.as_ref())
                            || active.as_ref() == Some(dialog.as_ref()))
                    {
                        event.prevent_default();
                        if let Err(error) = last.focus() {
                            web_sys::console::warn_2(
                                &"Could not move keyboard focus".into(),
                                &error,
                            );
                        }
                    } else if !event.shift_key() && active.as_ref() == Some(last.as_ref()) {
                        event.prevent_default();
                        if let Err(error) = first.focus() {
                            web_sys::console::warn_2(
                                &"Could not move keyboard focus".into(),
                                &error,
                            );
                        }
                    }
                } else {
                    event.prevent_default();
                    if let Err(error) = dialog.focus() {
                        web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error);
                    }
                }
            }),
    )
}
