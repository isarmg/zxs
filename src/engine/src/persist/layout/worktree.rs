//! Worktree instance identities are independent of reusable SMB display names.
use crate::directory_worktree::lifecycle::WorktreeId;
use platform_io::fs_safety::EntryName;

pub(crate) fn instance(id: WorktreeId) -> EntryName {
    EntryName::new(id.to_string())
        .expect("invariant: a worktree identity is one canonical ASCII entry")
}
