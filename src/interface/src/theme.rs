use gloo_events::EventListener;
use std::cell::{Cell, RefCell};
use wasm_bindgen::{JsCast, JsValue};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Theme {
    Light,
    Dark,
}
impl Theme {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
    fn system(query: Option<&web_sys::MediaQueryList>) -> Self {
        if query.is_some_and(|query| query.matches()) {
            Self::Dark
        } else {
            Self::Light
        }
    }
}

thread_local! {
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
    static MANUAL: Cell<bool> = const { Cell::new(false) };
    static SYSTEM_LISTENER: RefCell<Option<EventListener>> = const { RefCell::new(None) };
}

fn storage(window: &web_sys::Window) -> Option<web_sys::Storage> {
    match window.local_storage() {
        Ok(storage) => storage,
        Err(error) => {
            web_sys::console::warn_2(&"Theme preference storage is unavailable".into(), &error);
            None
        }
    }
}
fn saved(window: &web_sys::Window) -> Option<Theme> {
    let storage = storage(window)?;
    match storage.get_item("zxs-theme") {
        Ok(value) => value.as_deref().and_then(Theme::parse),
        Err(error) => {
            web_sys::console::warn_2(&"Could not read the theme preference".into(), &error);
            None
        }
    }
}
fn query(window: &web_sys::Window) -> Option<web_sys::MediaQueryList> {
    match window.match_media("(prefers-color-scheme: dark)") {
        Ok(query) => query,
        Err(error) => {
            web_sys::console::warn_2(&"System theme preference is unavailable".into(), &error);
            None
        }
    }
}
fn current_theme() -> Theme {
    let Some(window) = web_sys::window() else {
        return Theme::Light;
    };
    if let Some(theme) = window
        .document()
        .and_then(|document| document.document_element())
        .and_then(|root| root.get_attribute("data-zxs-theme"))
        .as_deref()
        .and_then(Theme::parse)
    {
        return theme;
    }
    saved(&window).unwrap_or_else(|| Theme::system(query(&window).as_ref()))
}
pub fn current() -> String {
    current_theme().as_str().into()
}
fn apply(theme: Theme) -> Result<(), JsValue> {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        root.set_attribute("data-zxs-theme", theme.as_str())?;
    }
    Ok(())
}
fn change(theme: Theme) {
    let result = apply(theme).and_then(|()| {
        if let Some(window) = web_sys::window() {
            window.dispatch_event(&web_sys::Event::new("zxs-theme-change")?)?;
        }
        Ok(())
    });
    if let Err(error) = result {
        web_sys::console::error_2(&"Could not change the theme".into(), &error);
    }
}

pub fn initialize() {
    if INITIALIZED.with(|initialized| initialized.replace(true)) {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let saved = saved(&window);
    MANUAL.with(|manual| manual.set(saved.is_some()));
    let query = query(&window);
    if let Err(error) = apply(saved.unwrap_or_else(|| Theme::system(query.as_ref()))) {
        web_sys::console::error_2(&"Could not initialize the theme".into(), &error);
    }
    if let Some(query) = query {
        let listener = EventListener::new(&query, "change", |event| {
            if MANUAL.with(Cell::get) {
                return;
            }
            if let Some(event) = event.dyn_ref::<web_sys::MediaQueryListEvent>() {
                change(if event.matches() {
                    Theme::Dark
                } else {
                    Theme::Light
                });
            }
        });
        SYSTEM_LISTENER.with(|listener_slot| *listener_slot.borrow_mut() = Some(listener));
    }
}

pub fn toggle() {
    let next = match current_theme() {
        Theme::Dark => Theme::Light,
        Theme::Light => Theme::Dark,
    };
    // Storage denial must not discard the user's document-local preference.
    MANUAL.with(|manual| manual.set(true));
    if let Some(window) = web_sys::window() {
        if let Some(storage) = storage(&window)
            && let Err(error) = storage.set_item("zxs-theme", next.as_str())
        {
            web_sys::console::warn_2(&"Could not save the theme preference".into(), &error);
        }
        change(next);
    }
}

/// Release the system preference listener when this document leaves its lifecycle.
#[cfg(target_arch = "wasm32")]
pub(crate) fn cleanup() {
    SYSTEM_LISTENER.with(|listener| listener.borrow_mut().take());
    INITIALIZED.with(|initialized| initialized.set(false));
}

#[cfg(test)]
#[path = "../../../test/interface/unit/theme/tests.rs"]
mod tests;
