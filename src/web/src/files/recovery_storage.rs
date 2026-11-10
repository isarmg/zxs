//! Shared browser I/O for immutable original-operation recovery records.
use crate::api;
use storage_contract::ErrorCode;

const MAX_STORAGE_KEYS: u32 = 4096;

pub(super) struct Store(web_sys::Storage);

fn failure(message: &str) -> api::Error {
    api::error(ErrorCode::InvalidRequest, message)
}

impl Store {
    pub(super) fn open() -> Result<Self, api::Error> {
        let unavailable = || failure("Browser recovery storage is unavailable.");
        web_sys::window()
            .ok_or_else(unavailable)?
            .local_storage()
            .map_err(api::js_error)?
            .map(Self)
            .ok_or_else(unavailable)
    }

    pub(super) fn scan(
        &self,
        prefix: &str,
        mut visit: impl FnMut(&str, &str) -> Result<(), api::Error>,
    ) -> Result<(), api::Error> {
        let count = self.0.length().map_err(api::js_error)?;
        if count > MAX_STORAGE_KEYS {
            return Err(api::error(
                ErrorCode::ResourceLimit,
                "Browser recovery storage exceeds its scan budget; no records were removed.",
            ));
        }
        for index in 0..count {
            let Some(key) = self.0.key(index).map_err(api::js_error)? else {
                continue;
            };
            if key.starts_with(prefix)
                && let Some(bytes) = self.0.get_item(&key).map_err(api::js_error)?
            {
                visit(&key, &bytes)?;
            }
        }
        Ok(())
    }

    pub(super) fn save(
        &self,
        key: &str,
        bytes: &str,
        matches: impl FnOnce(&str) -> Result<bool, api::Error>,
    ) -> Result<(), api::Error> {
        if self.0.get_item(key).map_err(api::js_error)?.is_some() {
            return Err(failure(
                "The recovery operation key is occupied; the original record was not overwritten.",
            ));
        }
        self.0.set_item(key, bytes).map_err(api::js_error)?;
        self.verify(key, matches)
    }

    pub(super) fn verify(
        &self,
        key: &str,
        matches: impl FnOnce(&str) -> Result<bool, api::Error>,
    ) -> Result<(), api::Error> {
        let Some(bytes) = self.0.get_item(key).map_err(api::js_error)? else {
            return Err(failure("The original recovery record is missing."));
        };
        if !matches(&bytes)? {
            return Err(failure("The original recovery record changed."));
        }
        Ok(())
    }

    pub(super) fn remove(
        &self,
        key: &str,
        matches: impl FnOnce(&str) -> Result<bool, api::Error>,
    ) -> Result<(), api::Error> {
        if let Some(bytes) = self.0.get_item(key).map_err(api::js_error)? {
            if !matches(&bytes)? {
                return Err(failure("The recovery record changed; it was not removed."));
            }
            self.0.remove_item(key).map_err(api::js_error)?;
        }
        if self.0.get_item(key).map_err(api::js_error)?.is_some() {
            return Err(failure(
                "Local recovery removal was not confirmed; the original record is retained.",
            ));
        }
        Ok(())
    }
}
