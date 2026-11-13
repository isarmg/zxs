//! A retained first-write intent is never a completed repository registration.
use platform_io::fs_safety::EntryName;
use std::sync::LazyLock;

pub(crate) static INTENT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("directory-snapshot-initialization.json")
        .expect("invariant: initialization intent is a fixed canonical entry")
});
