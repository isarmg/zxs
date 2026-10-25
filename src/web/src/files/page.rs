//! The browser retains one bounded page; cursors are opaque server values.
use crate::{api, format::parse_u64};
use json_codec::Value;

pub(super) const PAGE_SIZE: usize = 200;

pub(super) struct Page {
    pub next: Option<String>,
    pub previous: Option<String>,
    pub total: u64,
}
pub(super) fn selected(
    value: &Value,
    requested: Option<&Value>,
) -> Result<Option<Value>, api::Error> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    match value.get("selected_entry") {
        Some(Value::Null) => Ok(None),
        Some(entry)
            if entry.is_object()
                && entry["entry_id"] == requested["entry_id"]
                && entry["path"]
                    .as_str()
                    .zip(requested["path"].as_str())
                    .is_some_and(|(actual, expected)| {
                        name_policy::path_key(actual) == name_policy::path_key(expected)
                    }) =>
        {
            Ok(Some(entry.clone()))
        }
        _ => Err(api::Error {
            status: 200,
            code: "INVALID_DIRECTORY_PAGE".into(),
            message: "The selected entry did not match the requested identity.".into(),
            retryable: false,
            request_id: None,
        }),
    }
}

impl Page {
    pub fn parse(value: &Value) -> Result<Self, api::Error> {
        let invalid = || api::Error {
            status: 200,
            code: "INVALID_DIRECTORY_PAGE".into(),
            message: "The directory page did not match the server contract.".into(),
            retryable: false,
            request_id: None,
        };
        let cursor = |field: &str| -> Option<Option<String>> {
            match value.get(field)? {
                Value::Null => Some(None),
                Value::String(text) if !text.is_empty() => Some(Some(text.clone())),
                _ => None,
            }
        };
        let entries = value
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        let total = value
            .get("total_entries")
            .and_then(parse_u64)
            .ok_or_else(invalid)?;
        if entries.len() > PAGE_SIZE
            || total < entries.len() as u64
            || !value["snapshot"].as_str().is_some_and(|s| !s.is_empty())
        {
            return Err(invalid());
        }
        Ok(Self {
            next: cursor("next_cursor").ok_or_else(invalid)?,
            previous: cursor("previous_cursor").ok_or_else(invalid)?,
            total,
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/page/tests.rs"]
mod tests;
