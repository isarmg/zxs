//! Shared validation for user-authored metadata at both write and read boundaries.
use crate::{Error, Result};

pub const MAX_NOTE_BYTES: usize = 128;
pub const MAX_TAG_BYTES: usize = 128;
pub const MAX_FILE_TAGS: usize = 16;

pub fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty()
        || tag.trim() != tag
        || tag.len() > MAX_TAG_BYTES
        || tag.chars().any(char::is_control)
    {
        return Err(Error::new(
            crate::ErrorCode::InvalidTag,
            "tags must be nonempty, trimmed, control-free and at most 128 UTF-8 bytes",
        ));
    }
    Ok(())
}

/// Parsed user metadata; the empty value is an explicit clear command.
#[derive(Debug)]
pub struct Note(String);
impl Note {
    pub fn parse(value: String) -> Result<Self> {
        validate_note(&value)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Empty notes clear existing metadata. The limit counts UTF-8 bytes, and
/// control characters (including line breaks) are not part of the current format.
pub fn validate_note(note: &str) -> Result<()> {
    if note.len() > MAX_NOTE_BYTES || note.chars().any(char::is_control) {
        return Err(Error::new(
            crate::ErrorCode::InvalidRequest,
            format!(
                "notes must be at most {MAX_NOTE_BYTES} UTF-8 bytes without line breaks or control characters"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/contract/unit/text/tests.rs"]
mod tests;
