use crate::route::{Folder, Id};
use js_sys::{Array, Reflect, futures::JsFuture};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Request, RequestCache, RequestCredentials, RequestInit, RequestMode, RequestRedirect, Response, Window};

const MAX_CHILDREN: u32 = 32;
const MAX_ANCESTORS: u32 = 256;
const MAX_BODY: usize = 128 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Kind {
    Directory,
    File,
}

pub(crate) struct Entry {
    pub(crate) id: Id,
    pub(crate) name: String,
    pub(crate) kind: Kind,
}

pub(crate) struct Ancestor {
    pub(crate) id: Id,
    pub(crate) name: String,
}

pub(crate) struct Page {
    pub(crate) ancestors: Vec<Ancestor>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) next: Option<Id>,
}

#[derive(Clone, Copy)]
pub(crate) enum Failure {
    Expired,
    Changed,
    Unavailable,
}

pub(crate) async fn read(window: &Window, folder: Folder) -> Result<Page, Failure> {
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_mode(RequestMode::SameOrigin);
    init.set_credentials(RequestCredentials::SameOrigin);
    init.set_cache(RequestCache::NoStore);
    init.set_redirect(RequestRedirect::Error);
    let request = Request::new_with_str_and_init(&folder.request_path(), &init)
        .map_err(|_| Failure::Unavailable)?;
    let response = JsFuture::from(window.fetch_with_request(&request)).await
        .map_err(|_| Failure::Unavailable)?
        .dyn_into::<Response>().map_err(|_| Failure::Unavailable)?;
    match response.status() {
        200 => {}
        401 => return Err(Failure::Expired),
        403 | 404 | 409 => return Err(Failure::Changed),
        _ => return Err(Failure::Unavailable),
    }
    let headers = response.headers();
    let mime = headers.get("content-type").map_err(|_| Failure::Unavailable)?;
    let length = headers.get("content-length").map_err(|_| Failure::Unavailable)?
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or(Failure::Unavailable)?;
    if length > MAX_BODY || mime.as_deref().and_then(|s| s.split(';').next()) != Some("application/json") {
        return Err(Failure::Unavailable);
    }
    let json = JsFuture::from(response.text().map_err(|_| Failure::Unavailable)?).await
        .ok().and_then(|t| t.as_string()).ok_or(Failure::Unavailable)?;
    if json.len() > MAX_BODY {
        return Err(Failure::Unavailable);
    }
    let value = js_sys::JSON::parse(&json).map_err(|_| Failure::Unavailable)?;
    decode(&value, folder).ok_or(Failure::Unavailable)
}

fn member(value: &JsValue, name: &str) -> Option<JsValue> {
    Reflect::get(value, &JsValue::from_str(name)).ok()
}

fn id(value: &JsValue) -> Option<Id> {
    Id::parse(&member(value, "id")?.as_string()?)
}

fn name(value: &JsValue) -> Option<String> {
    let s = member(value, "name")?.as_string()?;
    (!s.is_empty() && s.len() <= 255 && !s.contains(['/', '\0'])).then_some(s)
}

fn bounded<T>(array: JsValue, limit: u32, parse: impl Fn(JsValue) -> Option<T>) -> Option<Vec<T>> {
    let array = array.dyn_into::<Array>().ok()?;
    (array.length() <= limit).then_some(())?;
    (0..array.length()).map(|i| parse(array.get(i))).collect()
}

fn decode(value: &JsValue, folder: Folder) -> Option<Page> {
    let ancestors = bounded(member(value, "ancestors")?, MAX_ANCESTORS, |row| {
        Some(Ancestor { id: id(&row)?, name: name(&row)? })
    })?;
    if ancestors.last().map(|row| row.id) != folder.parent {
        return None;
    }
    let entries = bounded(member(value, "entries")?, MAX_CHILDREN, |row| {
        let kind = match member(&row, "kind")?.as_string()?.as_str() {
            "directory" => Kind::Directory,
            "file" => Kind::File,
            _ => return None,
        };
        Some(Entry { id: id(&row)?, name: name(&row)?, kind })
    })?;
    let next = member(value, "next")?;
    let next = if next.is_null() { None } else { Some(Id::parse(&next.as_string()?)?) };
    if next.is_some() && (entries.is_empty() || next == folder.page) {
        return None;
    }
    Some(Page { ancestors, entries, next })
}
