//! Single-choice control sharing the application surface and contour renderer.
use crate::{i18n::t, prelude::*};
use gloo_events::{EventListener, EventListenerOptions};
use std::sync::atomic::{AtomicUsize, Ordering};
use wasm_bindgen::JsCast;

static SEQUENCE: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub disabled: bool,
}
impl SelectOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
        }
    }
}

fn next_enabled(options: &[SelectOption], current: Option<usize>, forward: bool) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let length = options.len();
    let start = current.filter(|index| *index < length);
    (1..=length)
        .map(|distance| match (start, forward) {
            (Some(index), true) => (index + distance) % length,
            (Some(index), false) => (index + length - distance % length) % length,
            (None, true) => distance - 1,
            (None, false) => length - distance,
        })
        .find(|index| !options[*index].disabled)
}

fn position(
    trigger: &web_sys::HtmlElement,
    panel: &web_sys::HtmlElement,
) -> Result<(), wasm_bindgen::JsValue> {
    let window = web_sys::window()
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("Browser window is unavailable"))?;
    let bounds = trigger.get_bounding_client_rect();
    let width = window
        .inner_width()?
        .as_f64()
        .expect("invariant: innerWidth is a browser number");
    let height = window
        .inner_height()?
        .as_f64()
        .expect("invariant: innerHeight is a browser number");
    let panel_width = bounds.width().max(180.0).min((width - 16.0).max(0.0));
    let left = bounds.left().min(width - panel_width - 8.0).max(8.0);
    let below = height - bounds.bottom() - 12.0;
    let above = bounds.top() - 12.0;
    let style = panel.style();
    style.set_property("width", &format!("{panel_width}px"))?;
    style.set_property("left", &format!("{left}px"))?;
    if below >= 180.0 || below >= above {
        style.set_property("max-height", &format!("{}px", below.clamp(44.0, 320.0)))?;
        style.set_property("top", &format!("{}px", bounds.bottom() + 4.0))?;
        style.set_property("bottom", "auto")?;
    } else {
        style.set_property("max-height", &format!("{}px", above.clamp(44.0, 320.0)))?;
        style.set_property("top", "auto")?;
        style.set_property("bottom", &format!("{}px", height - bounds.top() + 4.0))?;
    }
    Ok(())
}

/// A labeled, controlled single choice. Empty values are ordinary valid options.
/// Keyboard focus stays on the trigger; the active list option is announced by ID.
#[component]
pub fn Select(
    label: String,
    options: Signal<Vec<SelectOption>>,
    value: Signal<String>,
    on_change: Callback<String>,
    #[prop(default=Signal::derive(|| false))] disabled: Signal<bool>,
    #[prop(default=t("请选择", "Choose an option"))] placeholder: String,
) -> impl IntoView {
    let id = format!("zxs-select-{}", SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let list_id = StoredValue::new(format!("{id}-list"));
    let label_id = StoredValue::new(format!("{id}-label"));
    let control_id = StoredValue::new(id);
    let label = StoredValue::new(label);
    let placeholder = StoredValue::new(placeholder);
    let root = NodeRef::<web_interface::html::Div>::new();
    let trigger = NodeRef::<web_interface::html::Button>::new();
    let panel = NodeRef::<web_interface::html::Div>::new();
    let open = RwSignal::new(false);
    let active = RwSignal::new(None::<usize>);
    let prefix = StoredValue::new((String::new(), 0.0f64));
    let choose = move |index: usize| {
        let entries = options.get_untracked();
        if disabled.get_untracked() {
            return;
        }
        if let Some(option) = entries.get(index).filter(|option| !option.disabled) {
            open.set(false);
            on_change.run(option.value.clone());
        }
    };
    let expand = move || {
        if disabled.get_untracked() {
            return;
        }
        let entries = options.get_untracked();
        let selected = value.get_untracked();
        active.set(
            entries
                .iter()
                .position(|item| item.value == selected && !item.disabled)
                .or_else(|| next_enabled(&entries, None, true)),
        );
        open.set(true);
    };
    Effect::new(move |_| {
        let Some(panel) = panel.get() else {
            return;
        };
        if open.get() && !disabled.get() {
            if let Some(trigger) = trigger.get() {
                let result = position(trigger.unchecked_ref(), panel.unchecked_ref())
                    .and_then(|()| panel.show_popover());
                if let Err(error) = result {
                    web_sys::console::error_2(&"Could not open the selection menu".into(), &error);
                    open.set(false);
                }
            }
        } else {
            if let Err(error) = panel.hide_popover() {
                web_sys::console::warn_2(&"Could not close the selection menu".into(), &error);
            }
        }
    });
    Effect::new(move |_| {
        let entries = options.get();
        if disabled.get() && open.get() {
            open.set(false);
        }
        if open.get()
            && !active
                .get()
                .is_some_and(|index| entries.get(index).is_some_and(|item| !item.disabled))
        {
            let next = next_enabled(&entries, None, true);
            if active.get_untracked() != next {
                active.set(next);
            }
        }
    });
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some(index) = active.get() else {
            return;
        };
        let Some(panel) = panel.get() else {
            return;
        };
        let Some(option) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| {
                document.get_element_by_id(&format!("{}-{index}", list_id.get_value()))
            })
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        let top = option.offset_top();
        let bottom = top + option.offset_height();
        if top < panel.scroll_top() {
            panel.set_scroll_top(top);
        } else if bottom > panel.scroll_top() + panel.client_height() {
            panel.set_scroll_top(bottom - panel.client_height());
        }
    });
    if let Some(window) = web_sys::window() {
        let outside = EventListener::new(&window, "pointerdown", move |event| {
            if !open.get_untracked() {
                return;
            }
            let inside = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::Node>().ok())
                .is_some_and(|target| {
                    root.get_untracked()
                        .is_some_and(|root| root.contains(Some(&target)))
                });
            if !inside {
                open.set(false);
            }
        });
        let resize = EventListener::new(&window, "resize", move |_| open.set(false));
        let scroll = EventListener::new_with_options(
            &window,
            "scroll",
            EventListenerOptions::run_in_capture_phase(),
            move |event| {
                if !open.get_untracked() {
                    return;
                }
                let within_menu = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Node>().ok())
                    .is_some_and(|target| {
                        panel
                            .get_untracked()
                            .is_some_and(|panel| panel.contains(Some(&target)))
                    });
                if !within_menu {
                    open.set(false);
                }
            },
        );
        let listeners = send_wrapper::SendWrapper::new((outside, resize, scroll));
        on_cleanup(move || drop(listeners));
    }
    on_cleanup(move || {
        if let Some(panel) = panel.get_untracked()
            && let Err(error) = panel.hide_popover()
        {
            web_sys::console::warn_2(&"Could not close the selection menu".into(), &error);
        }
    });
    view! {
        <div class="zxs-select" node_ref=root>
            <label class="zxs-select-label zxs-row" id=label_id.get_value() for=control_id.get_value()>{label.get_value()}</label>
            <button class="zxs-input zxs-select-trigger zxs-input--compact" type="button" node_ref=trigger
                id=control_id.get_value() role="combobox" aria-haspopup="listbox"
                aria-labelledby=label_id.get_value() aria-controls=list_id.get_value()
                aria-expanded=move || open.get().to_string()
                aria-activedescendant=move || if open.get() { active.get().map(|index| format!("{}-{index}", list_id.get_value())) } else { None }
                disabled=move || disabled.get()
                on:click=move |_| { if open.get_untracked() { open.set(false); } else { expand(); } }
                on:blur=move |_| open.set(false)
                on:keydown=move |event: web_sys::KeyboardEvent| {
                    if disabled.get_untracked() || event.is_composing() { return; }
                    let entries = options.get_untracked();
                    match event.key().as_str() {
                        "Escape" => { if open.get_untracked() { event.prevent_default(); event.stop_propagation(); open.set(false); } }
                        "Tab" => open.set(false),
                        "ArrowDown" | "ArrowUp" => {
                            event.prevent_default();
                            if !open.get_untracked() { expand(); }
                            else { active.set(next_enabled(&entries, active.get_untracked(), event.key() == "ArrowDown")); }
                        }
                        "Home" | "End" => {
                            event.prevent_default();
                            if !open.get_untracked() { expand(); }
                            active.set(next_enabled(&entries, None, event.key() == "Home"));
                        }
                        "Enter" | " " => {
                            event.prevent_default();
                            if open.get_untracked() { if let Some(index) = active.get_untracked() { choose(index); } }
                            else { expand(); }
                        }
                        _ if !event.ctrl_key() && !event.meta_key() && !event.alt_key() && event.key().chars().count() == 1 => {
                            event.prevent_default();
                            if !open.get_untracked() { expand(); }
                            let now = js_sys::Date::now();
                            let (previous, when) = prefix.get_value();
                            let key = event.key().to_lowercase();
                            let search = if now - when < 700.0 { format!("{previous}{key}") } else { key };
                            prefix.set_value((search.clone(), now));
                            if let Some(index) = entries.iter().position(|item| !item.disabled && item.label.to_lowercase().starts_with(&search)) { active.set(Some(index)); }
                        }
                        _ => {}
                    }
                }>
                <span class="zxs-select-value">{move || {
                    options.with(|entries| {
                        entries.iter()
                            .find(|item| value.with(|value| item.value == *value))
                            .map(|item| item.label.clone())
                    }).unwrap_or_else(|| placeholder.get_value())
                }}</span>
                <svg class="zxs-select-arrow" aria-hidden="true" viewBox="0 0 16 16"><path d="m6 3 5 5-5 5" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>
            </button>
            <div class="zxs-select-panel zxs-frame" node_ref=panel id=list_id.get_value() role="listbox" popover="manual" aria-labelledby=label_id.get_value()>
                {move || {
                    let entries = options.get();
                    if entries.is_empty() { return view! { <div class="zxs-select-empty zxs-row" role="status">{t("没有可用选项", "No options available")}</div> }.into_any(); }
                    entries.into_iter().enumerate().map(move |(index, option)| {
                        let option_value = option.value.clone();
                        view! {
                            <div class="zxs-select-option zxs-row zxs-row--option" id=format!("{}-{index}", list_id.get_value()) role="option"
                                aria-selected=move || (value.get() == option_value).to_string()
                                aria-disabled=option.disabled.to_string()
                                data-active=move || (active.get() == Some(index)).to_string()
                                on:pointerdown=move |event: web_sys::PointerEvent| event.prevent_default()
                                on:mouseover=move |_| { if !option.disabled { active.set(Some(index)); } }
                                on:click=move |_| choose(index)>{option.label}</div>
                        }
                    }).collect_view().into_any()
                }}
            </div>
        </div>
    }
}

#[cfg(test)]
#[path = "../../../../test/interface/unit/components/select/tests.rs"]
mod tests;
