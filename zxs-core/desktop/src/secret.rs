//! Filter before egui Context receives raw native input. This does not control OS buffers.
use eframe::egui::{Event, ImeEvent, Key, RawInput};
use zeroize::{Zeroize, Zeroizing};

const MAX: usize = 1024;

pub struct Password {
    bytes: Zeroizing<[u8; MAX]>,
    used: usize,
}

impl Default for Password {
    fn default() -> Self {
        Self {
            bytes: Zeroizing::new([0; MAX]),
            used: 0,
        }
    }
}

#[derive(Default)]
pub struct Filtered {
    pub abort: bool,
    pub rejected: bool,
}

pub fn native_interrupted(raw: &RawInput) -> bool {
    let viewport = raw.viewport();
    !raw.focused
        || viewport.focused == Some(false)
        || viewport.minimized == Some(true)
        || viewport.occluded == Some(true)
        || viewport.close_requested()
        || raw.events.iter().any(|event| {
            matches!(
                event,
                Event::WindowFocused(false)
                    | Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        ..
                    }
            )
        })
}

impl Password {
    pub fn clear(&mut self) {
        self.bytes.zeroize();
        self.used = 0;
    }

    pub fn has_value(&self) -> bool {
        self.used != 0
    }

    fn append(&mut self, text: &str) -> bool {
        let Some(end) = self.used.checked_add(text.len()).filter(|x| *x <= MAX) else {
            return false;
        };
        if text.bytes().any(|b| b < 32 || b == 127) {
            return false;
        }
        self.bytes[self.used..end].copy_from_slice(text.as_bytes());
        self.used = end;
        true
    }

    fn erase(&mut self) {
        if let Some(from) = self.bytes[..self.used]
            .iter()
            .rposition(|b| b & 0b1100_0000 != 0b1000_0000)
        {
            self.bytes[from..self.used].zeroize();
            self.used = from;
        }
    }

    pub fn take(&mut self) -> Option<Zeroizing<String>> {
        let selected = if self.used == 0 {
            None
        } else {
            std::str::from_utf8(&self.bytes[..self.used])
                .ok()
                .map(|text| Zeroizing::new(text.to_owned()))
        };
        self.clear();
        selected
    }

    /// During review, setup, an armed prompt, handoff and retirement, nothing
    /// other than pointer events reaches egui. Focus/close/escape wins the
    /// entire coalesced batch, regardless of its order. IME and paste are not
    /// accepted in this small initial keyboard-only Linux slice.
    pub fn filter(&mut self, raw: &mut RawInput, capture: bool) -> Filtered {
        let stopped = native_interrupted(raw);
        let mut out = Filtered {
            abort: stopped,
            rejected: false,
        };
        if stopped || !capture {
            self.clear();
        }
        let incoming = std::mem::take(&mut raw.events);
        raw.hovered_files.clear();
        raw.dropped_files.clear();
        for event in incoming {
            match event {
                Event::Text(mut text) => {
                    if capture && !stopped && !out.rejected && !self.append(&text) {
                        self.clear();
                        out.rejected = true;
                    }
                    text.zeroize();
                }
                Event::Paste(mut text)
                | Event::MiddleClickPaste { mut text, .. }
                | Event::Ime(ImeEvent::Commit(mut text) | ImeEvent::Preedit { mut text, .. }) => {
                    text.zeroize();
                    if capture && !stopped {
                        self.clear();
                        out.rejected = true;
                    }
                }
                Event::Key {
                    key: Key::Backspace,
                    pressed: true,
                    ..
                } if capture && !stopped => self.erase(),
                e @ (Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone)
                    if !stopped =>
                {
                    raw.events.push(e)
                }
                _ => {} // Drops other physical/logical keys, copy/cut/paste images and all IME.
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Modifiers, Pos2, ViewportEvent};

    #[test]
    fn only_previously_armed_direct_text_survives_in_fixed_secret_storage() {
        let mut entry = Password::default();
        let mut raw = RawInput {
            events: vec![Event::Text("before-visible".to_owned())],
            ..RawInput::default()
        };
        entry.filter(&mut raw, false);
        assert!(!entry.has_value());
        assert!(raw.events.is_empty());

        raw.events = vec![
            Event::Text("sampleé".to_owned()),
            Event::Key {
                key: Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(Pos2::ZERO),
        ];
        let result = entry.filter(&mut raw, true);
        assert!(!result.abort && !result.rejected);
        assert_eq!(raw.events, vec![Event::PointerMoved(Pos2::ZERO)]);
        assert_eq!(entry.used, "sample".len());
        assert!(entry.bytes["sample".len()..].iter().all(|b| *b == 0));
        assert_eq!(&*entry.take().unwrap(), "sample");
        assert!(!entry.has_value());
        assert!(entry.bytes.iter().all(|b| *b == 0));
    }

    #[test]
    fn coalesced_focus_loss_or_paste_never_leaves_a_partial_credential() {
        let mut entry = Password::default();
        let mut input = RawInput {
            events: vec![
                Event::Text("first".to_owned()),
                Event::WindowFocused(false),
                Event::WindowFocused(true),
                Event::Text("last".to_owned()),
                Event::Ime(ImeEvent::Commit("other".to_owned())),
            ],
            ..RawInput::default()
        };
        assert!(entry.filter(&mut input, true).abort);
        assert!(input.events.is_empty() && !entry.has_value());
        input.events = vec![
            Event::Text("prefix".to_owned()),
            Event::Paste("foreign".to_owned()),
            Event::Text("suffix".to_owned()),
        ];
        assert!(entry.filter(&mut input, true).rejected);
        assert!(input.events.is_empty() && !entry.has_value());
    }

    #[test]
    fn minimize_close_or_escape_scrub_the_complete_batch() {
        for end in ["minimize", "occlude", "close", "escape"] {
            let mut entry = Password::default();
            assert!(entry.append("previous"));
            let mut input = RawInput {
                events: vec![
                    Event::Text("next-text".to_owned()),
                    Event::PointerMoved(Pos2::ZERO),
                ],
                ..RawInput::default()
            };
            let viewport = input.viewports.get_mut(&input.viewport_id).unwrap();
            match end {
                "minimize" => viewport.minimized = Some(true),
                "occlude" => viewport.occluded = Some(true),
                "close" => viewport.events.push(ViewportEvent::Close),
                _ => input.events.push(Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }),
            }
            assert!(entry.filter(&mut input, true).abort);
            assert!(input.events.is_empty());
            assert!(entry.bytes.iter().all(|b| *b == 0));
            assert!(entry.take().is_none());
        }
    }

    #[test]
    fn overlong_batch_is_not_truncated_and_left_available() {
        let mut entry = Password::default();
        let mut input = RawInput {
            events: vec![Event::Text("A".repeat(MAX)), Event::Text("B".to_owned())],
            ..RawInput::default()
        };
        assert!(entry.filter(&mut input, true).rejected);
        assert!(entry.take().is_none() && input.events.is_empty());
    }
}
