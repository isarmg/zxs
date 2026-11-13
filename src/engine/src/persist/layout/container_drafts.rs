//! Immutable shared-draft preparation evidence, not published version material.
use platform_io::fs_safety::EntryName;
use std::sync::LazyLock;

pub(crate) static ROOT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("container-drafts-v1").expect("invariant: fixed draft store entry")
});
pub(crate) static INTENT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("intent.json").expect("invariant: fixed draft intent entry")
});
pub(crate) static BODY: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("body").expect("invariant: fixed draft body entry")
});
pub(crate) static SCRATCH: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("body.partial").expect("invariant: fixed draft preparation entry")
});
