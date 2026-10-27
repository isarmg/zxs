//! Snapshot-bound selection; cursor validity never replaces current admission.
use super::*;
use crate::backend_listing::{ListingCursor, MAX_CURSOR_BYTES, SortKey};
pub(super) struct PageRequest {
    pub(super) limit: usize,
    query: String,
    snapshot: String,
    request: String,
    cursor: Option<ListingCursor>,
    selected: Option<(String, String)>,
}
impl PageRequest {
    pub(super) fn parse(engine: &Engine, input: crate::ReadListing) -> Result<Self, ApiError> {
        let snapshot = blake3::hash(&json_codec::to_vec(&json!({
            "configuration":engine.config(), "generation":engine.state().business_generation.to_string()
        })).expect("invariant: internal JSON serialization succeeds")).to_hex().to_string();
        Self::with_snapshot(input, snapshot)
    }
    pub(super) fn with_snapshot(
        input: crate::ReadListing,
        snapshot: String,
    ) -> Result<Self, ApiError> {
        let crate::ReadListing {
            slot_id,
            path,
            limit,
            query,
            cursor,
            selected,
        } = input;
        let path = path.as_ref().map(|path| path.key()).unwrap_or("");
        let request = blake3::hash(&json_codec::to_vec(&json!({
            "slot":slot_id,"path":path,"query":query,"limit":limit,
            "order":"directory-lowercase-normalized-name-path-identity", "unicode":name_policy::UNICODE_VERSION,"lowercase_unicode":std::char::UNICODE_VERSION
        })).expect("invariant: internal JSON serialization succeeds")).to_hex().to_string();
        if let Some(cursor) = &cursor {
            if cursor.request != request {
                return Err(ApiError::invalid("cursor does not match the listing query"));
            }
            if cursor.snapshot != snapshot {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::ListingChanged,
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
        let bytes = json_codec::to_vec(&ListingCursor {
            snapshot: self.snapshot.clone(),
            request: self.request.clone(),
            backward,
            key: key.clone(),
        })
        .expect("invariant: internal JSON serialization succeeds");
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write;
            write!(&mut encoded, "{byte:02x}").expect("invariant: formatting into String succeeds");
        }
        if encoded.len() > MAX_CURSOR_BYTES {
            return Err(ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::StateInvalid,
                "listing cursor exceeds its bounded representation",
            ));
        }
        Ok(encoded)
    }
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
        if self.request.selected.as_ref().is_some_and(|(path, id)| {
            *id == key.identity && *path == name_policy::path_key(&key.path)
        }) {
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
    pub(super) total: usize,
    pub(super) snapshot: String,
    pub(super) previous: Option<String>,
    pub(super) next: Option<String>,
}
#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/pagination_tests.rs"]
mod tests;
