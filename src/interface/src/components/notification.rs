use crate::prelude::*;
use gloo_timers::callback::Timeout;
use send_wrapper::SendWrapper;

const MAX_TOASTS: usize = 5;
const EXPIRY_MS: u32 = 5_000;

#[derive(Clone)]
pub(super) struct ToastRecord {
    pub(super) id: u64,
    pub(super) message: String,
}

/// Each visible record owns its cancellation handle. Eviction, dismissal and
/// owner disposal all release the same resource, rather than merely hiding it.
struct ToastQueue<T> {
    sequence: u64,
    entries: Vec<(ToastRecord, T)>,
}
impl<T> ToastQueue<T> {
    fn push(&mut self, message: String, timer: impl FnOnce(u64) -> T) {
        if self.entries.len() == MAX_TOASTS {
            self.entries.remove(0);
        }
        self.sequence = self.sequence.wrapping_add(1);
        let id = self.sequence;
        self.entries.push((
            ToastRecord {
                id,
                message: message.chars().take(512).collect(),
            },
            timer(id),
        ));
    }
    fn dismiss(&mut self, id: u64) {
        self.entries.retain(|(record, _)| record.id != id);
    }
    fn records(&self) -> Vec<ToastRecord> {
        self.entries
            .iter()
            .map(|(record, _)| record.clone())
            .collect()
    }
}

#[derive(Clone, Copy)]
pub(super) struct Toasts {
    pub(super) records: ReadSignal<Vec<ToastRecord>>,
    set_records: WriteSignal<Vec<ToastRecord>>,
    queue: StoredValue<SendWrapper<ToastQueue<Timeout>>>,
}
impl Toasts {
    pub(super) fn new() -> Self {
        let (records, set_records) = signal(Vec::new());
        Self {
            records,
            set_records,
            queue: StoredValue::new(SendWrapper::new(ToastQueue {
                sequence: 0,
                entries: Vec::new(),
            })),
        }
    }
    fn update(self, change: impl FnOnce(&mut ToastQueue<Timeout>)) {
        let records = self.queue.try_update_value(|queue| {
            change(queue);
            queue.records()
        });
        if let Some(records) = records {
            let _ = self.set_records.try_update(|value| *value = records);
        }
    }
    pub(super) fn dismiss(self, id: u64) {
        self.update(|queue| queue.dismiss(id));
    }
    pub(super) fn clear(self) {
        self.update(|queue| queue.entries.clear());
    }
}

pub fn notify(message: String) {
    // Async completions have no reactive Owner. Dispatch on the current
    // document rather than trying to resolve a context after an await.
    let Some(window) = web_sys::window() else {
        return;
    };
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&wasm_bindgen::JsValue::from_str(&message));
    let result = web_sys::CustomEvent::new_with_event_init_dict("zxs-notify", &init)
        .and_then(|event| window.dispatch_event(&event));
    if let Err(error) = result {
        web_sys::console::error_2(&"Could not deliver the notification".into(), &error);
    }
}
pub(super) fn push_toast(toasts: Toasts, message: String) {
    toasts.update(|queue| {
        queue.push(message, |id| {
            Timeout::new(EXPIRY_MS, move || toasts.dismiss(id))
        });
    });
}

#[cfg(test)]
#[path = "../../../../test/interface/unit/components/notification/tests.rs"]
mod tests;
