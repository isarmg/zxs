//! Stateless, snapshot-bound keyset pagination. Cursors select a position only;
//! every request still passes ordinary administrator admission.
use super::*;
use data_serialization::{Deserialize, Serialize};

pub(super) const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 500;
const MAX_CURSOR_BYTES: usize = 32_768;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct SortKey {
    file: bool,
    folded: String,
    normalized: String,
    name: String,
    path: String,
    identity: String,
}
impl SortKey {
    pub(super) fn new(directory: bool, name: &str, path: &str, identity: &str) -> Self {
        Self {
            file: !directory,
            folded: name.to_lowercase(),
            normalized: name_policy::lookup_key(name),
            name: name.into(),
            path: path.into(),
            identity: identity.into(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Cursor {
    snapshot: String,
    request: String,
    backward: bool,
    key: SortKey,
}
pub(super) struct PageRequest {
    pub(super) limit: usize,
    query: String,
    snapshot: String,
    request: String,
    cursor: Option<Cursor>,
    selected: Option<(String, String)>,
}
impl PageRequest {
    pub(super) fn parse(
        engine: &Engine,
        input: &Value,
        slot: &str,
        path: &str,
    ) -> Result<Self, ApiError> {
        let snapshot = blake3::hash(&json_codec::to_vec(&json!({
            "configuration":engine.config(), "generation":engine.state().business_generation.to_string()
        })).map_err(ApiError::invalid)?).to_hex().to_string();
        Self::with_snapshot(input, slot, path, snapshot)
    }
    pub(super) fn with_snapshot(
        input: &Value,
        slot: &str,
        path: &str,
        snapshot: String,
    ) -> Result<Self, ApiError> {
        let values = input.get("query");
        if let Some(values) = values {
            keys(
                values,
                &[
                    "path",
                    "query",
                    "limit",
                    "cursor",
                    "selected_path",
                    "selected_entry_id",
                ],
            )?;
        }
        let string = |name: &str| -> Result<Option<&str>, ApiError> {
            values
                .and_then(|values| values.get(name))
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| ApiError::invalid(format!("{name} must be a string")))
                })
                .transpose()
        };
        let limit = string("limit")?
            .map(|value| value.parse::<usize>().map_err(ApiError::invalid))
            .transpose()?
            .unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ApiError::invalid("limit must be between 1 and 500"));
        }
        let query = string("query")?.unwrap_or("");
        if query.len() > 1024 {
            return Err(ApiError::invalid("query exceeds 1024 UTF-8 bytes"));
        }
        let selected = match (string("selected_path")?, string("selected_entry_id")?) {
            (None, None) => None,
            (Some(selected_path), Some(id)) if !id.is_empty() && id.len() <= 256 => {
                name_policy::validate_name(selected_path.rsplit('/').next().unwrap_or_default())
                    .map_err(ApiError::invalid)?;
                let parent = selected_path
                    .rsplit_once('/')
                    .map(|(parent, _)| parent)
                    .unwrap_or("");
                if name_policy::path_key(parent) != name_policy::path_key(path) {
                    return Err(ApiError::invalid(
                        "selected entry must be a direct child of the requested directory",
                    ));
                }
                Some((selected_path.to_owned(), id.to_owned()))
            }
            _ => {
                return Err(ApiError::invalid(
                    "selected_path and selected_entry_id must be supplied together",
                ));
            }
        };
        let query = query.to_lowercase();
        let request = blake3::hash(&json_codec::to_vec(&json!({
            "slot":slot,"path":name_policy::path_key(path),"query":query,"limit":limit,
            "order":"directory-lowercase-normalized-name-path-identity", "unicode":name_policy::UNICODE_VERSION,"lowercase_unicode":std::char::UNICODE_VERSION
        })).map_err(ApiError::invalid)?).to_hex().to_string();
        let cursor = string("cursor")?.map(decode).transpose()?;
        if let Some(cursor) = &cursor {
            if cursor.request != request {
                return Err(ApiError::invalid("cursor does not match the listing query"));
            }
            if cursor.snapshot != snapshot {
                return Err(ApiError::conflict(
                    "LISTING_CHANGED",
                    "directory listing changed; restart from the first page",
                ));
            }
            if cursor.key
                != SortKey::new(
                    !cursor.key.file,
                    &cursor.key.name,
                    &cursor.key.path,
                    &cursor.key.identity,
                )
            {
                return Err(ApiError::invalid("cursor sort key is invalid"));
            }
        }
        Ok(Self {
            limit,
            query,
            snapshot,
            request,
            cursor,
            selected,
        })
    }
    fn backward(&self) -> bool {
        self.cursor.as_ref().is_some_and(|cursor| cursor.backward)
    }
    fn cursor(&self, key: &SortKey, backward: bool) -> Result<String, ApiError> {
        let bytes = json_codec::to_vec(&Cursor {
            snapshot: self.snapshot.clone(),
            request: self.request.clone(),
            backward,
            key: key.clone(),
        })
        .map_err(ApiError::invalid)?;
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write;
            write!(&mut encoded, "{byte:02x}").map_err(ApiError::invalid)?;
        }
        if encoded.len() > MAX_CURSOR_BYTES {
            return Err(ApiError::new(
                500,
                "STATE_INVALID",
                "listing cursor exceeds its bounded representation",
            ));
        }
        Ok(encoded)
    }
}
fn decode(value: &str) -> Result<Cursor, ApiError> {
    if value.is_empty() || value.len() > MAX_CURSOR_BYTES || !value.len().is_multiple_of(2) {
        return Err(ApiError::invalid("invalid listing cursor length"));
    }
    let bytes = value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(ApiError::invalid("invalid listing cursor encoding")),
            };
            Ok((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect::<Result<Vec<u8>, ApiError>>()?;
    json_codec::from_slice(&bytes).map_err(ApiError::invalid)
}

/// Retains at most limit + 1 lightweight candidates, independent of directory
/// cardinality. The current flat storage still requires a scan per request.
pub(super) struct PageSelector<T> {
    request: PageRequest,
    candidates: BTreeMap<SortKey, T>,
    total: usize,
    before: bool,
    after: bool,
    selected: Option<T>,
}
impl<T: Clone> PageSelector<T> {
    pub(super) fn new(request: PageRequest) -> Self {
        Self {
            request,
            candidates: BTreeMap::new(),
            total: 0,
            before: false,
            after: false,
            selected: None,
        }
    }
    pub(super) fn consider(&mut self, key: SortKey, value: T) {
        if self
            .request
            .selected
            .as_ref()
            .is_some_and(|(path, id)| *path == key.path && *id == key.identity)
        {
            self.selected = Some(value.clone());
        }
        if !key.folded.contains(&self.request.query) {
            return;
        }
        self.total += 1;
        if let Some(cursor) = &self.request.cursor {
            if self.request.backward() && key >= cursor.key {
                self.after = true;
                return;
            }
            if !self.request.backward() && key <= cursor.key {
                self.before = true;
                return;
            }
        }
        self.candidates.insert(key, value);
        if self.candidates.len() > self.request.limit + 1 {
            if self.request.backward() {
                self.candidates.pop_first();
            } else {
                self.candidates.pop_last();
            }
        }
    }
    pub(super) fn finish(mut self) -> Result<Page<T>, ApiError> {
        if self.candidates.len() > self.request.limit {
            if self.request.backward() {
                self.candidates.pop_first();
                self.before = true;
            } else {
                self.candidates.pop_last();
                self.after = true;
            }
        }
        let previous = self
            .candidates
            .first_key_value()
            .filter(|_| self.before)
            .map(|(key, _)| self.request.cursor(key, true))
            .transpose()?;
        let next = self
            .candidates
            .last_key_value()
            .filter(|_| self.after)
            .map(|(key, _)| self.request.cursor(key, false))
            .transpose()?;
        Ok(Page {
            values: self.candidates.into_values().collect(),
            total: self.total,
            snapshot: self.request.snapshot,
            previous,
            next,
            selected: self.selected,
        })
    }
}
pub(super) struct Page<T> {
    pub(super) values: Vec<T>,
    pub(super) selected: Option<T>,
    total: usize,
    snapshot: String,
    previous: Option<String>,
    next: Option<String>,
}
impl<T> Page<T> {
    pub(super) fn metadata(&self, response: &mut Value) {
        response["total_entries"] = self.total.to_string().into();
        response["snapshot"] = self.snapshot.clone().into();
        response["previous_cursor"] = self.previous.clone().into();
        response["next_cursor"] = self.next.clone().into();
    }
}
#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/pagination_tests.rs"]
mod tests;
