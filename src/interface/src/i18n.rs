use std::cell::RefCell;
use wasm_bindgen::JsValue;
use web_sys::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Locale {
    Chinese,
    English,
}
impl Locale {
    fn as_str(self) -> &'static str {
        match self {
            Self::Chinese => "zh-CN",
            Self::English => "en",
        }
    }
}

thread_local! { static LOCALE: RefCell<Option<Locale>> = const { RefCell::new(None) }; }

fn supported(value: &str) -> Option<Locale> {
    let mut parts = value.split('-');
    let first = parts.next()?.to_ascii_lowercase();
    if parts.any(|part| {
        !(2..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphanumeric())
    }) {
        return None;
    }
    match first.as_str() {
        "zh" => Some(Locale::Chinese),
        "en" => Some(Locale::English),
        _ => None,
    }
}

/// Whether a locale can be normalized to one of the supported interface languages.
pub fn valid_locale(value: &str) -> bool {
    supported(value).is_some()
}

fn current() -> Locale {
    LOCALE.with(|saved| {
        if let Some(value) = saved.borrow().as_ref() {
            return *value;
        }
        let value = resolve();
        *saved.borrow_mut() = Some(value);
        value
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve() -> Locale {
    Locale::English
}

pub fn locale() -> String {
    current().as_str().into()
}

fn location_url(window: &web_sys::Window) -> Result<Url, JsValue> {
    Url::new(&window.location().href()?)
}

#[cfg(target_arch = "wasm32")]
fn resolve() -> Locale {
    let Some(window) = web_sys::window() else {
        return Locale::English;
    };
    match location_url(&window) {
        Ok(url) => {
            if let Some(value) = url
                .search_params()
                .get("lang")
                .as_deref()
                .and_then(supported)
            {
                return value;
            }
        }
        Err(error) => web_sys::console::warn_2(&"Could not read the language URL".into(), &error),
    }
    let saved = window.local_storage().and_then(|storage| match storage {
        Some(storage) => storage.get_item("zxs-language"),
        None => Ok(None),
    });
    match saved {
        Ok(value) => {
            if let Some(locale) = value.as_deref().and_then(supported) {
                return locale;
            }
        }
        Err(error) => {
            web_sys::console::warn_2(&"Could not read the language preference".into(), &error)
        }
    }
    for value in window
        .navigator()
        .languages()
        .iter()
        .filter_map(|v| v.as_string())
    {
        if let Some(value) = supported(&value) {
            return value;
        }
    }
    Locale::English
}

pub fn t(zh: &str, en: &str) -> String {
    match current() {
        Locale::Chinese => zh,
        Locale::English => en,
    }
    .to_owned()
}

pub fn tf(zh: &str, en: &str, values: &[&str]) -> String {
    let template = t(zh, en);
    let mut text = String::with_capacity(template.len());
    let mut remaining = template.as_str();
    // Read only the template, never replacement values. Preserve unknown and
    // noncanonical placeholders exactly as the former literal replacement did.
    while let Some(start) = remaining.find('{') {
        text.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        if let Some(end) = remaining.find('}') {
            let token = &remaining[1..end];
            if let Ok(index) = token.parse::<usize>()
                && token == index.to_string()
                && let Some(value) = values.get(index)
            {
                text.push_str(value);
                remaining = &remaining[end + 1..];
                continue;
            }
        }
        text.push('{');
        remaining = &remaining[1..];
    }
    text.push_str(remaining);
    text
}

pub fn initialize() {
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Some(root) = window
        .document()
        .and_then(|document| document.document_element())
        && let Err(error) = root.set_attribute("lang", current().as_str())
    {
        web_sys::console::warn_2(&"Could not set the document language".into(), &error);
    }
    let save = || -> Result<(), JsValue> {
        let url = location_url(&window)?;
        if url
            .search_params()
            .get("lang")
            .as_deref()
            .and_then(supported)
            .is_some()
            && let Some(storage) = window.local_storage()?
        {
            storage.set_item("zxs-language", current().as_str())?;
        }
        Ok(())
    };
    if let Err(error) = save() {
        web_sys::console::warn_2(&"Could not save the language preference".into(), &error);
    }
}

pub fn switch_language() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let result = location_url(&window).and_then(|url| {
        let next = match current() {
            Locale::Chinese => Locale::English,
            Locale::English => Locale::Chinese,
        };
        url.search_params().set("lang", next.as_str());
        window.location().assign(&url.href())
    });
    if let Err(error) = result {
        web_sys::console::error_2(&"Could not switch the language".into(), &error);
    }
}

#[cfg(test)]
#[path = "../../../test/interface/unit/i18n/tests.rs"]
mod tests;
