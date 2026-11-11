//! Stable entry names shared by runtime publication and recovery.
use platform_io::fs_safety::EntryName;
use std::sync::LazyLock;

pub(crate) static DISK_IDENTITY: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("disk.json").expect("invariant: disk identity uses one canonical entry")
});

/// Serializes writers of a system or control directory.
pub(crate) static CONTROL_WRITER_LOCK: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("writer.lock").expect("invariant: control writer lock uses one canonical entry")
});

/// Fences writers of a managed disk root independently of the control lock.
pub(crate) static DISK_WRITER_LOCK: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new(".writer.lock").expect("invariant: disk writer lock uses one canonical entry")
});

pub(crate) static JOURNAL: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("journal.bin").expect("invariant: journal uses one canonical entry")
});

pub(crate) static PROJECTION_KIND: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("projection-kind.json")
        .expect("invariant: projection kind uses one canonical entry")
});

pub(crate) static PROJECTION_CACHE: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("cache.db").expect("invariant: projection cache uses one canonical entry")
});

pub(crate) static PROJECTION_CACHE_LOCK: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("cache.lock").expect("invariant: projection cache lock uses one canonical entry")
});

pub(crate) fn projection_cache_staging(process: u32, sequence: u64) -> EntryName {
    EntryName::new(format!("cache-build-{process}-{sequence}.db"))
        .expect("invariant: decimal process and sequence form one canonical cache entry")
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/persist/layout_tests.rs"]
mod tests;
