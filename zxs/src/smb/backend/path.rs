//! Share-relative spellings, not host paths or capabilities. No string is
//! mapped to an F/R role, authenticated identity, volume, object or permission
//! here. The original tree must select every component from current Core state.
use crate::{display, smb::wire::{message::Message, path_fields::FieldError}};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum NameError { Field(FieldError), InvalidName, TooLong, Resources }

impl From<FieldError> for NameError {
    fn from(value: FieldError) -> Self { Self::Field(value) }
}

/// A nonempty exact spelling within one tree. Splitting borrows one bounded
/// allocation; a long path does not allocate a String or Vec for each name.
/// Deliberately no AsRef<Path>, raw join, Unicode normalization or case folding.
pub(in crate::smb) struct TreeName(String);

impl TreeName {
    /// Reuse the same grammar on the bounded relative bytes of a checked
    /// current CREATE. Its caller must still allocate the actual object.
    pub(in crate::smb) fn create_relative(raw:&[u8])->Result<Self,NameError>{Self::decode(raw,NameUse::Create)}

    pub(in crate::smb) fn components(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.0.split('\\')
    }

    fn decode(raw: &[u8], use_: NameUse) -> Result<Self, NameError> {
        use NameError::*;
        if raw.is_empty() || !raw.len().is_multiple_of(2) { return Err(InvalidName); }
        let units = raw.len() / 2;
        if units > 32_760 { return Err(TooLong); } // MS-FSCC 2.1.5; bounded UTF-16 storage
        // For rename one leading separator can designate THIS tree root.
        // Never consume a UNC/device prefix, collapse multiple separators,
        // substitute for malformed scalars or normalize dot components.
        let input = if use_ == NameUse::RenameTarget {
            raw.strip_prefix(&[b'\\', 0]).unwrap_or(raw)
        } else { raw };
        let mut text = String::new();
        text.try_reserve_exact(3 * units).map_err(|_| Resources)?;
        let words = input.as_chunks::<2>().0.iter().copied().map(u16::from_le_bytes);
        for c in char::decode_utf16(words) {
            text.push(c.map_err(|_| InvalidName)?);
        }
        for component in text.split('\\') {
            display::component(component.as_bytes(), display::MAX).map_err(|_| {
                if component.len() > display::MAX { TooLong } else { InvalidName }
            })?;
        }
        Ok(Self(text))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum NameUse { Create, RenameTarget }

pub(in crate::smb) struct CreateSpelling<'a> {
    /// None is the protocol's empty name. It can denote the tree root for a
    /// normal CREATE, but is not a root capability (e.g. durable reconnect).
    pub(in crate::smb) name: Option<TreeName>,
    /// Still unprocessed. No command may use its path to skip processing an
    /// accompanying reconnect request, lease, or other CREATE context.
    pub(in crate::smb) context_area: &'a [u8],
}

impl<'a> CreateSpelling<'a> {
    pub(in crate::smb) fn from_message(m: &Message<'a>) -> Result<Self, NameError> {
        let fields = m.create_name_fields()?;
        let name = if fields.name.is_empty() { None } else {
            Some(TreeName::decode(fields.name, NameUse::Create)?)
        };
        Ok(Self { name, context_area: fields.contexts })
    }
}

pub(in crate::smb) struct RenameSpelling {
    pub(in crate::smb) source_wire_id: [u8; 16],
    pub(in crate::smb) destination: TreeName,
    pub(in crate::smb) requested_replace: bool,
    pub(in crate::smb) had_root_separator: bool,
}

impl RenameSpelling {
    /// ZXS accepts a target path in the source's same tree. This product
    /// spelling does not select a restore operation; MS-SMB2 3.3.5.21.1's
    /// separator wording and native clients' actual requests require separate
    /// interop verification. The restore path must never overwrite a collision,
    /// irrespective of the client's replacement preference for general rename.
    pub(in crate::smb) fn from_message(m: &Message<'_>) -> Result<Self, NameError> {
        let fields = m.basic_rename_name_field()?;
        Ok(Self { source_wire_id: fields.source_wire_id, requested_replace: fields.replace_if_exists,
                  destination: TreeName::decode(fields.name, NameUse::RenameTarget)?,
                  had_root_separator: fields.name.starts_with(&[b'\\', 0]) })
    }
}

#[cfg(test)]
#[path = "../../test/smb/backend_path.rs"]
mod tests;
