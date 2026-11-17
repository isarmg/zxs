//! One immutable pending request per account; it contains no credentials or body data.
use super::wire::Record;
use crate::api;
use crate::files::recovery_storage::Store;
use storage_contract::Principal;

fn key(principal: &Principal) -> String {
    format!("zxs:first-file-commit:{}", api::segment(principal.as_str()))
}
pub(super) fn load(principal: &Principal) -> Result<Option<Record>, api::Error> {
    let key = key(principal);
    let mut record = None;
    Store::open()?.scan(&key, |found, bytes| {
        if found == key {
            record = Some(Record::parse(bytes, principal)?);
        }
        Ok(())
    })?;
    Ok(record)
}
pub(super) fn save(record: &Record) -> Result<(), api::Error> {
    let bytes = record.encode()?;
    Store::open()?.save(&key(&record.principal), &bytes, |stored| {
        Ok(Record::parse(stored, &record.principal)? == *record)
    })
}
pub(super) fn verify(record: &Record) -> Result<(), api::Error> {
    Store::open()?.verify(&key(&record.principal), |stored| {
        Ok(Record::parse(stored, &record.principal)? == *record)
    })
}
pub(super) fn remove(record: &Record) -> Result<(), api::Error> {
    Store::open()?.remove(&key(&record.principal), |stored| {
        Ok(Record::parse(stored, &record.principal)? == *record)
    })
}
