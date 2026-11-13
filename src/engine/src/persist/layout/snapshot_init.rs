//! A retained first-write intent is never a completed repository registration.
use platform_io::fs_safety::EntryName;
use std::sync::LazyLock;

pub(crate) static INTENT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("directory-snapshot-initialization.json")
        .expect("invariant: initialization intent is a fixed canonical entry")
});

pub(crate) static DECISION: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("initialization-decision.json").expect("fixed initialization proof name")
});

pub(crate) fn stage(operation: storage_contract::OperationId) -> EntryName {
    EntryName::new(format!("snapshot-init-{operation}"))
        .expect("typed operation identity is a bounded portable entry")
}
