//! Bound an ordinary HTTP error before it can leave its native job permit.
//! CLI diagnostics and large, explicitly-owned response bodies are separate.
use std::{error::Error, fmt::{self, Write}};

pub(super) const MAX_MESSAGE_BYTES: usize = 4096;
const MAX_CAUSES: usize = 8;
const SHORTENED: &str = "…";

#[derive(Debug)]
pub(super) struct SmallDiagnostic(String);
impl SmallDiagnostic {
    pub(super) fn display(value: impl fmt::Display) -> Self {
        let mut output = Limited::default();
        if write!(output, "{value}").is_err() { output.shorten(); }
        Self(output.text)
    }

    pub(super) fn chain(error: &(dyn Error + 'static)) -> Self {
        let mut output = Limited::default();
        let mut current = Some(error);
        for _ in 0..MAX_CAUSES {
            let Some(cause) = current else { return Self(output.text); };
            if !output.text.is_empty() && output.write_str("; caused by: ").is_err()
                || write!(output, "{cause}").is_err()
            {
                output.shorten();
                return Self(output.text);
            }
            current = cause.source();
        }
        if current.is_some() { output.shorten(); }
        Self(output.text)
    }

    pub(super) fn into_string(self) -> String { self.0 }
}

#[derive(Default)]
struct Limited {
    text: String,
    stopped: bool,
    // Last complete UTF-8 character or escaped control that leaves marker room.
    marker_at: usize,
}
impl Limited {
    fn shorten(&mut self) {
        if self.stopped { return; }
        self.text.truncate(self.marker_at);
        self.text.push_str(SHORTENED);
        self.stopped = true;
    }
}
impl Write for Limited {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.stopped { return Err(fmt::Error); }
        for character in value.chars() {
            let width = if character.is_control() {
                character.escape_default().count()
            } else {
                character.len_utf8()
            };
            if width > MAX_MESSAGE_BYTES - self.text.len() {
                self.shorten();
                return Err(fmt::Error);
            }
            if character.is_control() {
                self.text.extend(character.escape_default());
            } else {
                self.text.push(character);
            }
            if self.text.len() <= MAX_MESSAGE_BYTES - SHORTENED.len() {
                self.marker_at = self.text.len();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/console/small_diagnostic_tests.rs"]
mod tests;
