//! Node material lives beside worktrees, never inside a mutable worktree instance.
use platform_io::fs_safety::EntryName;
use std::sync::LazyLock;
use crate::working_tree::snapshot::SnapshotPayload;
use storage_contract::working_tree::SnapshotNodeId;

pub(crate) static MANIFEST: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("manifest.json").expect("invariant: snapshot manifest is one canonical entry")
});
pub(crate) static FILES: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("files").expect("invariant: snapshot files is one canonical entry")
});
pub(crate) static WORKTREES: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("worktrees").expect("invariant: snapshot worktrees is one canonical entry")
});
pub(crate) fn node(id: SnapshotNodeId) -> EntryName {
    EntryName::new(id.to_string())
        .expect("invariant: snapshot node identity is one canonical ASCII entry")
}
pub(crate) fn file(payload: &SnapshotPayload) -> EntryName {
    EntryName::new(format!("{}-{}", payload.storage_id, payload.source_path.file_name()))
        .expect("invariant: admitted snapshot storage identity and basename form one entry")
}
