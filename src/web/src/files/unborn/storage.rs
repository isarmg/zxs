//! One immutable pending request per account; it contains no credentials or body data.
use super::wire::Record;
use crate::api;
use crate::files::recovery_storage::Store;
use storage_contract::Principal;

fn key(principal: &Principal) -> String {
    format!("zxs:first-file-commit:{}", api::segment(principal.as_str()))
}

/// Retain the actual UTF-8 text as well as its admitted meaning: compare-and-remove
/// must not consume externally replaced bytes, even when the new encoding is equivalent.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct Original {
    pub(super) request: Record,
    bytes: Box<str>,
}
impl Original {
    pub(super) fn create(request: Record) -> Result<Self, api::Error> {
        let bytes = request.encode()?.into_boxed_str();
        Ok(Self { request, bytes })
    }

    pub(super) fn read(bytes: &str, principal: &Principal) -> Result<Self, api::Error> {
        Ok(Self {
            request: Record::parse(bytes, principal)?,
            bytes: bytes.into(),
        })
    }
}

pub(super) fn load(principal: &Principal) -> Result<Option<Original>, api::Error> {
    let key = key(principal);
    let mut original = None;
    Store::open()?.scan(&key, |found, bytes| {
        if found == key {
            original = Some(Original::read(bytes, principal)?);
        }
        Ok(())
    })?;
    Ok(original)
}

/// Constructed only by the same-origin lock callback for this exact account key.
pub(super) struct Slot(String);

pub(super) async fn while_held<F>(
    principal: &Principal,
    action: impl FnOnce(Slot) -> F + 'static,
) -> Result<(), api::Error>
where
    F: std::future::Future<Output = Result<(), api::Error>> + 'static,
{
    crate::browser_lock::while_held(key(principal), |key| action(Slot(key))).await
}

impl Slot {
    pub(super) fn save(&self, original: &Original) -> Result<(), api::Error> {
        Store::open()?.save(&self.0, &original.bytes, |stored| {
            Ok(stored == original.bytes.as_ref())
        })
    }
    pub(super) fn verify(&self, original: &Original) -> Result<(), api::Error> {
        Store::open()?.verify(&self.0, |stored| Ok(stored == original.bytes.as_ref()))
    }
    pub(super) fn remove(&self, original: &Original) -> Result<(), api::Error> {
        Store::open()?.remove(&self.0, |stored| Ok(stored == original.bytes.as_ref()))
    }
}
