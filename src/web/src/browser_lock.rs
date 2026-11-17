//! Serialize cooperating same-origin writers without changing retained bytes or keys.
use crate::api;
use std::{cell::RefCell, future::Future, panic::AssertUnwindSafe, rc::Rc};
use storage_contract::ErrorCode;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_interface::i18n::t;

fn unavailable() -> api::Error {
    api::error(
        ErrorCode::RecoveryRequired,
        t(
            "此浏览器需要 Web Locks 才能跨标签页保存原请求；未尝试无锁写入。",
            "Web Locks are required to preserve the original request; no unlocked write was attempted.",
        ),
    )
}

/// Immediate actions (e.g. backup intent replacement) do not hold the lock across I/O.
pub(crate) async fn exclusive(
    key: String,
    action: impl FnOnce(&str) -> Result<(), api::Error> + 'static,
) -> Result<(), api::Error> {
    while_held(key, move |key| std::future::ready(action(&key))).await
}

/// The request callback returns the action's promise. The browser holds the
/// same-origin key until the action settles (or the document is destroyed).
pub(crate) async fn while_held<F>(
    key: String,
    action: impl FnOnce(String) -> F + 'static,
) -> Result<(), api::Error>
where
    F: Future<Output = Result<(), api::Error>> + 'static,
{
    let window = web_sys::window().ok_or_else(unavailable)?;
    let navigator = js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("navigator"))
        .map_err(api::js_error)?;
    let locks =
        js_sys::Reflect::get(&navigator, &JsValue::from_str("locks")).map_err(api::js_error)?;
    if locks.is_null() || locks.is_undefined() {
        return Err(unavailable());
    }
    let request = js_sys::Reflect::get(&locks, &JsValue::from_str("request"))
        .map_err(api::js_error)?
        .dyn_into::<js_sys::Function>()
        .map_err(api::js_error)?;
    let name = JsValue::from_str(&key);
    let result = Rc::new(RefCell::new(None));
    let completed = Rc::clone(&result);
    let mut grant = Some((key, action));
    // The grant is consumed once. If JS observes an unwind, the promise rejects
    // and no completion is treated as proof that a mutation was safely rejected.
    let callback = Closure::wrap_assert_unwind_safe(Box::new(move |_lock: JsValue| {
        let Some((key, action)) = grant.take() else {
            return js_sys::Promise::reject(&JsValue::from_str("duplicate Web Lock grant")).into();
        };
        let task = action(key);
        let completed = Rc::clone(&completed);
        // A JS rejection cannot establish a successful storage or network outcome.
        js_sys::futures::future_to_promise(AssertUnwindSafe(async move {
            let outcome = task.await;
            *completed.borrow_mut() = Some(outcome);
            Ok(JsValue::UNDEFINED)
        }))
        .into()
    }) as Box<dyn FnMut(JsValue) -> JsValue>);
    let promise = request
        .call2(&locks, &name, callback.as_ref())
        .map_err(api::js_error)?
        .dyn_into::<js_sys::Promise>()
        .map_err(api::js_error)?;
    let granted = js_sys::futures::JsFuture::from(promise).await;
    drop(callback);
    granted.map_err(api::js_error)?;
    result.take().ok_or_else(unavailable)?
}
