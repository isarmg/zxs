//! Fixed wire examples are not proof of live ownership or complete source membership.
use crate::{Result, require, text};
use json_codec::Value;
use storage_contract::file_lifecycle::{Cursor, MAX_EVENTS, MAX_RESPONSE_BYTES, Page, Request};

pub(crate) fn validate(name: &str, value: &Value) -> Result {
    if name == "FileLifecycleCursor" {
        Cursor::parse(text(value)?)?;
        return Ok(());
    }
    let bytes = json_codec::to_vec(value)?;
    require(
        bytes.len() <= MAX_RESPONSE_BYTES,
        "lifecycle example exceeds byte budget",
    )?;
    let page: Page = storage_contract::decode_strict(&bytes)?;
    // Fixed vectors use the maximum page limit and have no originating cursor.
    // Runtime consumers separately bind their exact request, context and prior member.
    let request = Request::parse(
        page.file_id.as_str(),
        &page.history_root.to_string(),
        None,
        None,
        None,
        MAX_EVENTS,
    )?;
    page.validate(&request)?;
    Ok(())
}
