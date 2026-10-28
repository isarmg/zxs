//! One admission boundary for the modern browser contract, before any UI mounts.
use wasm_bindgen::{JsCast, JsValue};
use web_sys::Window;

fn method(owner: &JsValue, name: &str) -> Result<js_sys::Function, JsValue> {
    let value = js_sys::Reflect::get(owner, &name.into())?;
    if !value.is_function() {
        return Err(JsValue::from_str(&format!(
            "Unsupported browser: required {name} is unavailable"
        )));
    }
    Ok(value.unchecked_into())
}

pub(crate) fn require(view: &Window) -> Result<(), JsValue> {
    // Do not maintain UA/version tables or install replacements. Constructors
    // are checked here, not in each component or contour paint frame.
    for name in [
        "ResizeObserver",
        "MutationObserver",
        "requestAnimationFrame",
        "cancelAnimationFrame",
        "matchMedia",
        "AbortController",
    ] {
        method(view.as_ref(), name)?;
    }
    // The request boundary identifies cancellation by this exact reason, not
    // by an AbortError name or by undefined equalling an absent reason getter.
    let controller = web_sys::AbortController::new()?;
    let reason = JsValue::from(js_sys::Object::new());
    controller.abort_with_reason(&reason);
    let signal = controller.signal();
    if !signal.aborted() || signal.reason() != reason {
        return Err(JsValue::from_str(
            "Unsupported browser: required AbortSignal.reason identity is unavailable",
        ));
    }
    let encoder = js_sys::Reflect::get(view, &"TextEncoder".into())?;
    let prototype = js_sys::Reflect::get(&encoder, &"prototype".into())?;
    method(&prototype, "encodeInto")?;
    let css = js_sys::Reflect::get(view, &"CSS".into())?;
    method(&css, "registerProperty")?;
    let supports = method(&css, "supports")?;
    for condition in [
        "(mask-image: url(\"blob:zxs-support-probe\"))",
        "(clip-path: path(\"M0 0H1V1Z\"))",
        "selector(:has(*))",
        "(container-type: inline-size)",
    ] {
        if supports.call1(&css, &condition.into())?.as_bool() != Some(true) {
            return Err(JsValue::from_str(&format!(
                "Unsupported browser: required CSS {condition} is unavailable"
            )));
        }
    }
    Ok(())
}
