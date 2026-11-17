//! Per-operation keys retain every original when separate tabs race. Scanning
//! is bounded. Every cooperating writer below holds the same account Web Lock.
//! The original server still decides; a browser lock is never admission authority.
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

/// Preserve the actual validated text. Two encodings of the same command are
/// not the same local evidence: one must never authorize removal of the other.
#[derive(Eq, PartialEq)]
pub(super) struct Original {
    request: Record,
    bytes: Box<str>,
}
impl Original {
    pub(super) fn fresh(request: Record) -> Result<Self, api::Error> {
        let bytes = request.encode()?.into_boxed_str();
        Ok(Self { request, bytes })
    }
    pub(super) fn matches(&self, bytes: &str) -> bool {
        self.bytes.as_ref() == bytes
    }
}
impl std::ops::Deref for Original {
    type Target = Record;
    fn deref(&self) -> &Record {
        &self.request
    }
}

/// The UI displays a count and works on one original. Validate every input,
/// but never retain 32 full request bodies (and 32 raw 256 KiB copies) merely
/// to choose the first. No observation is a reserve or permission to mutate.
#[derive(Default)]
pub(super) struct Inventory {
    seen: std::collections::BTreeSet<storage_contract::OperationId>,
    preferred: Option<storage_contract::OperationId>,
    pub(super) first: Option<Arc<Original>>,
}
impl Inventory {
    pub(super) fn preferring(preferred: Option<storage_contract::OperationId>) -> Self {
        Self {
            preferred,
            ..Self::default()
        }
    }
    pub(super) fn count(&self) -> usize {
        self.seen.len()
    }
    pub(super) fn observe(
        &mut self,
        key: &str,
        bytes: &str,
        principal: &Principal,
    ) -> Result<(), api::Error> {
        if self.count() == MAX_RECORDS {
            return Err(invalid(
                "directory recovery exceeds 32 operations; nothing was removed",
            ));
        }
        let request = decode(key, bytes, principal)?;
        let operation = request.command.operation();
        if !self.seen.insert(operation) {
            return Err(invalid(
                "directory recovery changed during observation; nothing was removed",
            ));
        }
        if self.first.as_ref().is_none_or(|current| {
            let active = current.command.operation();
            Some(operation) == self.preferred
                || (Some(active) != self.preferred && operation < active)
        }) {
            self.first = Some(Arc::new(Original {
                request,
                bytes: bytes.into(),
            }));
        }
        Ok(())
    }
}
pub(super) fn load(
    principal: &Principal,
    preferred: Option<storage_contract::OperationId>,
) -> Result<Inventory, api::Error> {
    let mut inventory = Inventory::preferring(preferred);
    Store::open()?.scan(&prefix(principal), |key, bytes| {
        inventory.observe(key, bytes, principal)
    })?;
    Ok(inventory)
}

/// All per-operation keys in this account share an admission slot so the
/// prefix scan and first no-other-original check cannot race a second tab.
/// Existing independent records retain their actual per-operation keys.
pub(super) struct Slot(String);
pub(super) async fn while_held<F>(
    principal: &Principal,
    action: impl FnOnce(Slot) -> F + 'static,
) -> Result<(), api::Error>
where
    F: std::future::Future<Output = Result<(), api::Error>> + 'static,
{
    crate::browser_lock::while_held(prefix(principal), |key| action(Slot(key))).await
}
impl Slot {
    fn require(&self, original: &Original) -> Result<(), api::Error> {
        if self.0 != prefix(&original.principal) {
            return Err(invalid("directory journal lock belongs to another account"));
        }
        Ok(())
    }
    pub(super) fn save(&self, original: &Original) -> Result<(), api::Error> {
        self.require(original)?;
        if load(&original.principal, None)?.count() != 0 {
            return Err(invalid(
                "recover the retained directory request before starting another",
            ));
        }
        Store::open()?.save(&key(original), &original.bytes, |stored| {
            Ok(original.matches(stored))
        })
    }
    pub(super) fn verify(&self, original: &Original) -> Result<(), api::Error> {
        self.require(original)?;
        Store::open()?.verify(&key(original), |stored| Ok(original.matches(stored)))
    }
    pub(super) fn remove(&self, original: &Original) -> Result<(), api::Error> {
        self.require(original)?;
        Store::open()?.remove(&key(original), |stored| Ok(original.matches(stored)))
    }
}
