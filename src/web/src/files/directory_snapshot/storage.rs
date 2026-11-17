//! Per-operation keys retain every original when separate tabs race. Scanning
//! is bounded; localStorage is not a cross-tab transaction or a server lock.
use crate::api;
use crate::files::directory_snapshot::wire::{Record, invalid};
use crate::files::recovery_storage::Store;
use std::sync::Arc;
use storage_contract::Principal;

const MAX_RECORDS: usize = 32;

fn prefix(principal: &Principal) -> String {
    format!(
        "zxs:directory-snapshot:{}:",
        api::segment(principal.as_str())
    )
}

pub(super) fn key(record: &Record) -> String {
    format!(
        "{}{}",
        prefix(&record.principal),
        record.command.operation()
    )
}

pub(super) fn decode(key: &str, bytes: &str, principal: &Principal) -> Result<Record, api::Error> {
    let record = Record::parse(bytes, principal)?;
    if self::key(&record) != key {
        return Err(invalid(
            "the original request differs from its immutable storage key",
        ));
    }
    Ok(record)
}

pub(super) fn load(principal: &Principal) -> Result<Vec<Arc<Record>>, api::Error> {
    let mut records = Vec::new();
    Store::open()?.scan(&prefix(principal), |key, bytes| {
        if records.len() == MAX_RECORDS {
            return Err(invalid(
                "directory recovery exceeds 32 operations; nothing was removed",
            ));
        }
        records.push(Arc::new(decode(key, bytes, principal)?));
        Ok(())
    })?;
    records.sort_by_key(|record| record.command.operation());
    Ok(records)
}

pub(super) fn save(record: &Record) -> Result<(), api::Error> {
    if !load(&record.principal)?.is_empty() {
        return Err(invalid(
            "recover the retained directory request before starting another",
        ));
    }
    let bytes = record.encode()?;
    Store::open()?.save(&key(record), &bytes, |stored| {
        Ok(Record::parse(stored, &record.principal)? == *record)
    })
}

pub(super) fn verify(record: &Record) -> Result<(), api::Error> {
    Store::open()?.verify(&key(record), |stored| {
        Ok(Record::parse(stored, &record.principal)? == *record)
    })
}

pub(super) fn remove(record: &Record) -> Result<(), api::Error> {
    Store::open()?.remove(&key(record), |stored| {
        Ok(Record::parse(stored, &record.principal)? == *record)
    })
}
