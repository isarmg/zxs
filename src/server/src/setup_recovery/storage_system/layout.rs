use platform_io::fs_safety::{EntryName, RelativePath};
use std::sync::LazyLock;

pub(super) static SYSTEM: LazyLock<RelativePath> = LazyLock::new(|| {
    RelativePath::new("storage-system").expect("invariant: fixed private SYSTEM child name")
});

pub(super) static CONFIG: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("config.json").expect("invariant: fixed storage configuration name")
});
