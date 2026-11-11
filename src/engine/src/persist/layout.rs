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

#[cfg(test)]
#[path = "../../../../test/engine/unit/persist/layout_tests.rs"]
mod tests;
