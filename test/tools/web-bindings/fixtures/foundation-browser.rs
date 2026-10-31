//! Targeted consumer contracts executed in real browser WASM, not host stand-ins.
use gloo_events::EventListener;
use gloo_timers::callback::{Interval, Timeout};
use next_tuple::NextTuple;
use or_poisoned::OrPoisoned;
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex, RwLock},
};
use wasm_bindgen::{prelude::*, JsCast};

#[derive(Default)]
struct Hook(Mutex<Vec<String>>);
impl throw_error::ErrorHook for Hook {
    fn throw(&self, error: throw_error::Error) -> throw_error::ErrorId {
        let mut values = self.0.lock().or_poisoned();
        values.push(error.to_string());
        (values.len() - 1).into()
    }
    fn clear(&self, id: &throw_error::ErrorId) {
        self.0.lock().or_poisoned().push(format!("clear:{id}"));
    }
}

fn value_contracts() {
    let (name, number, owned) =
        ().next_tuple("tuple")
            .next_tuple(7)
            .next_tuple(String::from("owned"));
    assert_eq!((name, number, owned.as_str()), ("tuple", 7, "owned"));
    let lock = RwLock::new(7);
    *lock.write().or_poisoned() = 11;
    assert_eq!(*lock.read().or_poisoned(), 11);
    let mutex = Mutex::new(String::from("lock"));
    mutex.lock().or_poisoned().push_str(" value");
    assert_eq!(*mutex.lock().or_poisoned(), "lock value");

    let mut slots = slotmap::SlotMap::new();
    let removed = slots.insert(String::from("old"));
    assert_eq!(slots.remove(removed).as_deref(), Some("old"));
    let current = slots.insert(String::from("new"));
    assert_ne!(removed, current);
    assert!(slots.get(removed).is_none());
    assert_eq!(&slots[current], "new");
    let mut secondary = slotmap::SecondaryMap::new();
    secondary.insert(current, 42);
    assert_eq!(secondary.get(current), Some(&42));
    assert_eq!(secondary.get(removed), None);

    const CONCAT: [u8; 4096] = const_str_slice_concat::const_concat(&["文件", "", "Ω"]);
    assert_eq!(const_str_slice_concat::str_from_buffer(&CONCAT), "文件Ω");
    const WRAPPED: [u8; 4096] =
        const_str_slice_concat::const_concat_with_prefix(&["a", "b"], "[", "]");
    assert_eq!(const_str_slice_concat::str_from_buffer(&WRAPPED), "[ab]");
    const CLASSES: [u8; 4096] =
        const_str_slice_concat::const_concat_with_separator(&["a", "", "bc"], " ");
    assert_eq!(const_str_slice_concat::str_from_buffer(&CLASSES), "a bc");

    assert!(throw_error::get_error_hook().is_none());
    let outer = Arc::new(Hook::default());
    let outer_guard = throw_error::set_error_hook(outer.clone());
    let id = throw_error::throw(std::io::Error::other("outer"));
    throw_error::clear(&id);
    let inner = Arc::new(Hook::default());
    {
        let _inner_guard = throw_error::set_error_hook(inner.clone());
        throw_error::throw(std::io::Error::other("inner"));
    }
    throw_error::throw(std::io::Error::other("restored"));
    assert_eq!(
        *outer.0.lock().or_poisoned(),
        ["outer", "clear:0", "restored"]
    );
    assert_eq!(*inner.0.lock().or_poisoned(), ["inner"]);
    drop(outer_guard);
    assert!(throw_error::get_error_hook().is_none());
}

async fn wait_for_timer() -> Result<(), JsValue> {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        Timeout::new(30, move || {
            resolve.call0(&JsValue::UNDEFINED).unwrap();
        })
        .forget();
    });
    js_sys::futures::JsFuture::from(promise).await?;
    Ok(())
}

#[wasm_bindgen]
pub async fn run_foundation_probe() -> Result<String, JsValue> {
    value_contracts();
    let object = js_sys::Object::new();
    js_sys::Reflect::set(&object, &"value".into(), &"文件Ω".into())?;
    assert_eq!(
        js_sys::Reflect::get(&object, &"value".into())?
            .as_string()
            .as_deref(),
        Some("文件Ω")
    );
    let array = js_sys::Array::new();
    array.push(&object);
    assert!(js_sys::Object::is(&array.get(0), &object));
    assert_eq!(array.length(), 1);
    let document = web_sys::window().unwrap().document().unwrap();
    let element = document.create_element("div")?;
    element.set_attribute("data-probe", "<文件>")?;
    element.set_text_content(Some("<b>文字</b>"));
    assert_eq!(
        element.get_attribute("data-probe").as_deref(),
        Some("<文件>")
    );
    assert_eq!(element.children().length(), 0);
    let target: &web_sys::EventTarget = element.unchecked_ref();
    let calls = Rc::new(Cell::new(0));
    let count = Rc::clone(&calls);
    let listener = EventListener::new(target, "foundation-event", move |_| {
        count.set(count.get() + 1)
    });
    target.dispatch_event(&web_sys::Event::new("foundation-event")?)?;
    assert_eq!(calls.get(), 1);
    drop(listener);
    target.dispatch_event(&web_sys::Event::new("foundation-event")?)?;
    assert_eq!(calls.get(), 1);
    let count = Rc::clone(&calls);
    let once = EventListener::once(target, "foundation-once", move |_| {
        count.set(count.get() + 1)
    });
    for _ in 0..2 {
        target.dispatch_event(&web_sys::Event::new("foundation-once")?)?;
    }
    assert_eq!(calls.get(), 2);
    drop(once);

    let completed = Rc::new(Cell::new(0));
    let count = Rc::clone(&completed);
    let canceled = Timeout::new(0, move || count.set(99));
    drop(canceled);
    let count = Rc::clone(&completed);
    let active = Timeout::new(0, move || count.set(count.get() + 1));
    wait_for_timer().await?;
    assert_eq!(completed.get(), 1);
    drop(active);
    let ticks = Rc::new(Cell::new(0));
    let count = Rc::clone(&ticks);
    let interval = Interval::new(1, move || count.set(count.get() + 1));
    wait_for_timer().await?;
    assert!(ticks.get() > 0);
    drop(interval);
    let stopped = ticks.get();
    wait_for_timer().await?;
    assert_eq!(ticks.get(), stopped);
    Ok("{\"status\":\"passed\",\"checks\":[\"next_tuple values and ownership\",\"or_poisoned real lock guards\",\"slotmap stale key generation\",\"const concatenation and class separators\",\"throw_error nested hook restoration\",\"js-sys values and identity\",\"web-sys DOM text and event target\",\"gloo-events once and drop cleanup\",\"gloo-timers fire and cancel cleanup\"]}".into())
}

#[wasm_bindgen]
pub fn run_foundation_panic_probe() {
    console_error_panic_hook::set_once();
    console_error_panic_hook::set_once();
    panic!("foundation-console-panic-witness");
}

#[cfg(test)]
mod tests {
    #[test]
    fn separators_skip_empty_fragments_and_preserve_unicode() {
        for (fragments, separator, expected) in [
            (&[][..], "::", ""),
            (&["", "", ""][..], "::", ""),
            (&["", "文件", "", "Ω", ""][..], "::", "文件::Ω"),
            (&["a", "b"][..], "", "ab"),
        ] {
            let value = const_str_slice_concat::const_concat_with_separator(fragments, separator);
            assert_eq!(const_str_slice_concat::str_from_buffer(&value), expected);
        }
    }

    #[test]
    fn concatenated_class_tokens_keep_separator() {
        const VALUE: [u8; 4096] =
            const_str_slice_concat::const_concat_with_separator(&["a", "", "bc"], " ");
        assert_eq!(const_str_slice_concat::str_from_buffer(&VALUE), "a bc");
    }
}
