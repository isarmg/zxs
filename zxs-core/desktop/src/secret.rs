//! Protected input is consumed before egui sees it. OS/IME memory is separate.
use eframe::egui::{Event, ImeEvent, Key, Modifiers, RawInput};
use zeroize::{Zeroize, Zeroizing};

const MAX: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFrame {
    Active,
    Inactive,
    Cancel,
    Close,
}

impl NativeFrame {
    pub fn from_raw(raw: &RawInput) -> Self {
        let viewport = raw.viewport();
        if viewport.close_requested() {
            return Self::Close;
        }
        if !raw.focused
            || viewport.focused == Some(false)
            || viewport.minimized == Some(true)
            || viewport.occluded == Some(true)
            || raw
                .events
                .iter()
                .any(|e| matches!(e, Event::WindowFocused(false)))
        {
            return Self::Inactive;
        }
        if raw.events.iter().any(|e| {
            matches!(
                e,
                Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                }
            )
        }) {
            return Self::Cancel;
        }
        Self::Active
    }

    pub fn active(self) -> bool {
        self == Self::Active
    }

    pub fn explicit_end(self) -> bool {
        matches!(self, Self::Cancel | Self::Close)
    }
}

pub fn clipboard_chord(modifiers: Modifiers) -> bool {
    modifiers.command && !modifiers.shift && !modifiers.alt
}

pub struct Password {
    bytes: Zeroizing<[u8; MAX]>,
    used: usize,
    enter_down: bool,
    paste_down: bool,
    composing: bool,
    foreground: bool,
}

impl Default for Password {
    fn default() -> Self {
        Self {
            bytes: Zeroizing::new([0; MAX]),
            used: 0,
            enter_down: false,
            paste_down: false,
            composing: false,
            foreground: false,
        }
    }
}

#[derive(Default)]
pub struct Filtered {
    pub submit: bool,
    pub rejected: bool,
    pub paste_requested: bool,
}

impl Password {
    pub fn clear(&mut self) {
        self.clear_text();
        self.enter_down = false;
        self.paste_down = false;
        self.composing = false;
        self.foreground = false;
    }

    fn clear_text(&mut self) {
        self.bytes.zeroize();
        self.used = 0;
    }

    pub fn has_value(&self) -> bool {
        self.used != 0
    }

    pub fn ready_to_submit(&self) -> bool {
        self.has_value() && !self.composing
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
        let out = if self.used == 0 {
            None
        } else {
            std::str::from_utf8(&self.bytes[..self.used])
                .ok()
                .map(|text| Zeroizing::new(text.to_owned()))
        };
        self.clear();
        out
    }

    /// Transient focus loss stops new input and clears composition, not the
    /// committed draft. Explicit cancellation, close and context changes wipe.
    pub fn filter(
        &mut self,
        raw: &mut RawInput,
        shown_prompt: bool,
        frame: NativeFrame,
        paste_pending: bool,
    ) -> Filtered {
        if frame.explicit_end() {
            self.clear();
        }
        let capture = shown_prompt && frame.active();
        let ime_batch = raw.events.iter().any(|event| {
            matches!(
                event,
                Event::Ime(ImeEvent::Preedit { .. } | ImeEvent::Commit(_))
            )
        });
        // The native adapter doesn't preserve the OS repeat flag in RawInput.
        // A held key and the focus-return/composition frame cannot submit.
        let permit_shortcuts = capture && self.foreground && !self.composing && !ime_batch;
        self.foreground = frame.active();
        if !capture {
            self.composing = false;
        }
        let incoming = std::mem::take(&mut raw.events);
        raw.hovered_files.clear();
        raw.dropped_files.clear();
        let mut out = Filtered::default();
        for event in incoming {
            match event {
                Event::Text(mut text) => {
                    if capture
                        && !self.composing
                        && !ime_batch
                        && !out.rejected
                        && !out.submit
                        && !self.append(&text)
                    {
                        self.clear_text();
                        out.rejected = true;
                    }
                    text.zeroize();
                }
                Event::Ime(ImeEvent::Preedit { mut text, .. }) => {
                    self.composing = capture && !text.is_empty();
                    text.zeroize();
                }
                Event::Ime(ImeEvent::Commit(mut text)) => {
                    self.composing = false;
                    if capture && !out.rejected && !out.submit && !self.append(&text) {
                        self.clear_text();
                        out.rejected = true;
                    }
                    text.zeroize();
                }
                Event::Ime(_) => self.composing = false,
                Event::Paste(mut text) => {
                    // The native clipboard gate already matched the exact
                    // request; no protected input ever reaches a TextEdit.
                    if capture && !out.rejected && !out.submit && !self.append(&text) {
                        self.clear_text();
                        out.rejected = true;
                    }
                    text.zeroize();
                }
                Event::MiddleClickPaste { mut text, .. } => text.zeroize(),
                Event::Key {
                    key: Key::Enter,
                    pressed,
                    modifiers,
                    ..
                } => {
                    if pressed
                        && !self.enter_down
                        && permit_shortcuts
                        && !paste_pending
                        && !out.paste_requested
                        && self.ready_to_submit()
                        && modifiers == Modifiers::NONE
                    {
                        out.submit = true;
                    }
                    self.enter_down = pressed;
                }
                Event::Key {
                    key: Key::V,
                    pressed,
                    modifiers,
                    ..
                } => {
                    if pressed
                        && !self.paste_down
                        && permit_shortcuts
                        && !out.submit
                        && clipboard_chord(modifiers)
                    {
                        out.paste_requested = true;
                    }
                    self.paste_down = pressed;
                }
                Event::Key {
                    key: Key::Backspace,
                    pressed: true,
                    ..
                } if capture && !self.composing && !ime_batch && !out.submit => self.erase(),
                e @ (Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone)
                    if frame.active() =>
                {
                    raw.events.push(e)
                }
                _ => {}
            }
        }
        if out.rejected {
            out.submit = false;
            out.paste_requested = false;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    fn filter(entry: &mut Password, raw: &mut RawInput, shown: bool) -> Filtered {
        let frame = NativeFrame::from_raw(raw);
        entry.filter(raw, shown, frame, false)
    }

    #[test]
    fn only_previously_armed_direct_text_survives_in_fixed_secret_storage() {
        let mut entry = Password::default();
        let mut raw = RawInput {
            events: vec![Event::Text("before-visible".to_owned())],
            ..RawInput::default()
        };
        filter(&mut entry, &mut raw, false);
        assert!(!entry.has_value() && raw.events.is_empty());
        raw.events = vec![
            Event::Text("sampleé".to_owned()),
            key(Key::Backspace, true, Modifiers::NONE),
        ];
        filter(&mut entry, &mut raw, true);
        assert!(raw.events.is_empty());
        assert_eq!(&*entry.take().unwrap(), "sample");
        assert!(entry.bytes.iter().all(|b| *b == 0));
    }

    #[test]
    fn focus_loss_preserves_only_the_prior_draft() {
        let mut entry = Password::default();
        let mut raw = RawInput::default();
        assert!(entry.append("kept"));
        raw.events = vec![
            Event::Text("focus-before".into()),
            Event::WindowFocused(false),
            Event::WindowFocused(true),
            Event::Ime(ImeEvent::Commit("not added".into())),
        ];
        assert!(!filter(&mut entry, &mut raw, true).submit);
        raw.events = vec![Event::Text("!".into())];
        filter(&mut entry, &mut raw, true);
        assert!(raw.events.is_empty());
        assert_eq!(&*entry.take().unwrap(), "kept!");
    }

    #[test]
    fn hide_preserves_but_close_or_escape_wipes_the_draft_and_batch() {
        for end in ["minimize", "occlude", "close", "escape"] {
            let mut entry = Password::default();
            assert!(entry.append("previous"));
            let mut raw = RawInput {
                events: vec![Event::Text("not added".into())],
                ..RawInput::default()
            };
            let viewport = raw.viewports.get_mut(&raw.viewport_id).unwrap();
            match end {
                "minimize" => viewport.minimized = Some(true),
                "occlude" => viewport.occluded = Some(true),
                "close" => viewport.events.push(eframe::egui::ViewportEvent::Close),
                _ => raw.events.push(key(Key::Escape, true, Modifiers::NONE)),
            }
            assert!(!filter(&mut entry, &mut raw, true).submit);
            assert!(raw.events.is_empty());
            if matches!(end, "close" | "escape") {
                assert!(!entry.has_value() && entry.bytes.iter().all(|b| *b == 0));
            } else {
                assert_eq!(&*entry.take().unwrap(), "previous");
            }
        }
    }

    #[test]
    fn overlong_batch_is_not_truncated_and_left_available() {
        let mut entry = Password::default();
        let mut raw = RawInput {
            events: vec![Event::Text("A".repeat(MAX)), Event::Text("B".into())],
            ..RawInput::default()
        };
        assert!(filter(&mut entry, &mut raw, true).rejected);
        assert!(entry.take().is_none() && raw.events.is_empty());
    }

    #[test]
    fn only_new_enter_after_composition_can_submit() {
        let mut entry = Password::default();
        let mut raw = RawInput::default();
        filter(&mut entry, &mut raw, true);
        raw.events = vec![
            Event::Ime(ImeEvent::Commit("口令".into())),
            key(Key::Enter, true, Modifiers::NONE),
        ];
        assert!(!filter(&mut entry, &mut raw, true).submit);
        raw.events = vec![key(Key::Enter, true, Modifiers::NONE)]; // raw backend also reports repeat: false
        assert!(!filter(&mut entry, &mut raw, true).submit);
        raw.events = vec![
            key(Key::Enter, false, Modifiers::NONE),
            key(Key::Enter, true, Modifiers::NONE),
        ];
        assert!(filter(&mut entry, &mut raw, true).submit);
        assert_eq!(&*entry.take().unwrap(), "口令");
    }

    #[test]
    fn only_explicit_foreground_paste_is_requested_and_enter_ends_that_batch() {
        let mut entry = Password::default();
        let mut raw = RawInput::default();
        let ctrl = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };
        // Previous native foreground frame rendered the prompt.
        filter(&mut entry, &mut raw, false);
        raw.events = vec![key(Key::V, true, ctrl)];
        assert!(filter(&mut entry, &mut raw, true).paste_requested);
        raw.events = vec![Event::WindowFocused(false), key(Key::V, true, ctrl)];
        assert!(!filter(&mut entry, &mut raw, true).paste_requested);
        filter(&mut entry, &mut raw, true);
        raw.events = vec![
            key(Key::V, false, ctrl),
            key(Key::V, true, ctrl),
            key(Key::V, true, ctrl),
        ];
        assert!(filter(&mut entry, &mut raw, true).paste_requested);
        raw.events = vec![Event::Paste("approved".into())]; // already matched by native queue
        filter(&mut entry, &mut raw, true);
        raw.events = vec![
            key(Key::Enter, true, Modifiers::NONE),
            Event::Text("ignored suffix".into()),
        ];
        assert!(filter(&mut entry, &mut raw, true).submit);
        assert!(raw.events.is_empty());
        assert_eq!(&*entry.take().unwrap(), "approved");
    }
}
