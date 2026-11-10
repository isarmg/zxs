//! Raw JSON exists only while checking the exact response shape.
use super::{DecodeError, DisplayPath};
use json_codec::{Map, Value};

pub(super) struct Object(Map<String, Value>);
impl Object {
    pub(super) fn parse(value: Value, field: &'static str) -> Result<Self, DecodeError> {
        match value {
            Value::Object(fields) => Ok(Self(fields)),
            _ => Err(field_error(field, "an object")),
        }
    }
    pub(super) fn take(&mut self, field: &'static str) -> Result<Value, DecodeError> {
        self.0.remove(field).ok_or(DecodeError::Missing(field))
    }
    pub(super) fn contains(&self, field: &'static str) -> bool {
        self.0.contains_key(field)
    }
    pub(super) fn text(&mut self, field: &'static str) -> Result<String, DecodeError> {
        match self.take(field)? {
            Value::String(text) => Ok(text),
            _ => Err(field_error(field, "a string")),
        }
    }
    pub(super) fn id(&mut self, field: &'static str) -> Result<String, DecodeError> {
        let id = self.text(field)?;
        if id.is_empty() || id.len() > 256 {
            return Err(field_error(
                field,
                "a nonempty identity of at most 256 bytes",
            ));
        }
        Ok(id)
    }
    pub(super) fn boolean(&mut self, field: &'static str) -> Result<bool, DecodeError> {
        match self.take(field)? {
            Value::Bool(value) => Ok(value),
            _ => Err(field_error(field, "a boolean")),
        }
    }
    // The current server omits false backup markers; this is not a default for
    // required fields or a compatibility format.
    pub(super) fn backup(&mut self) -> Result<bool, DecodeError> {
        match self.0.remove("backup") {
            None => Ok(false),
            Some(Value::Bool(true)) => Ok(true),
            Some(_) => Err(field_error("backup", "true when present")),
        }
    }
    pub(super) fn nullable_text(
        &mut self,
        field: &'static str,
    ) -> Result<Option<String>, DecodeError> {
        match self.take(field)? {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text)),
            _ => Err(field_error(field, "a string or null")),
        }
    }
    pub(super) fn nullable_id(
        &mut self,
        field: &'static str,
    ) -> Result<Option<String>, DecodeError> {
        let value = self.nullable_text(field)?;
        if value
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err(field_error(
                field,
                "a nonempty identity of at most 256 bytes or null",
            ));
        }
        Ok(value)
    }
    pub(super) fn integer(&mut self, field: &'static str) -> Result<u64, DecodeError> {
        let text = self.text(field)?;
        if text.is_empty()
            || (text.len() > 1 && text.starts_with('0'))
            || !text.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(field_error(field, "a canonical decimal u64 string"));
        }
        text.parse()
            .map_err(|source| DecodeError::Integer { field, source })
    }
    pub(super) fn array(&mut self, field: &'static str) -> Result<Vec<Value>, DecodeError> {
        match self.take(field)? {
            Value::Array(items) => Ok(items),
            _ => Err(field_error(field, "an array")),
        }
    }
    pub(super) fn finish(self) -> Result<(), DecodeError> {
        match self.0.into_iter().next() {
            Some((field, _)) => Err(DecodeError::Unknown(field)),
            None => Ok(()),
        }
    }
}
pub(super) fn field_error(field: &'static str, expected: &'static str) -> DecodeError {
    DecodeError::Field { field, expected }
}
pub(super) fn lookup_path(
    text: &str,
    field: &'static str,
) -> Result<Option<name_policy::LookupPath>, DecodeError> {
    if text.is_empty() {
        return Ok(None);
    }
    name_policy::LookupPath::parse(text)
        .map(Some)
        .map_err(|source| DecodeError::Path { field, source })
}
pub(super) fn portable_path(
    text: &str,
    field: &'static str,
) -> Result<Option<name_policy::PortablePath>, DecodeError> {
    if text.is_empty() {
        return Ok(None);
    }
    name_policy::PortablePath::parse(text)
        .map(Some)
        .map_err(|source| DecodeError::Path { field, source })
}
pub(super) fn display_path(text: String) -> Result<DisplayPath, DecodeError> {
    // Primary listings may prepend a repository name to a bounded namespace
    // path. Validate every component without imposing the namespace-only budget.
    for component in text.split('/') {
        name_policy::validate_name(component).map_err(|source| DecodeError::Path {
            field: "path",
            source,
        })?;
    }
    Ok(DisplayPath {
        key: name_policy::path_key(&text),
        text,
    })
}
pub(super) fn direct_child(entry: &super::Entry, directory_key: &str) -> bool {
    entry
        .path
        .key
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent)
        == directory_key
}
