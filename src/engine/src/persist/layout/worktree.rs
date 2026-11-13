//! Worktree instance identities are independent of reusable SMB display names.
use crate::directory_worktree::lifecycle::WorktreeId;
use platform_io::fs_safety::EntryName;

pub(crate) fn instance(id: WorktreeId) -> EntryName {
    EntryName::new(id.to_string())
        .expect("invariant: a worktree identity is one canonical ASCII entry")
}

/// Ledger material is a sibling, never inside the initially empty instance.
pub(crate) fn ledger(id: WorktreeId) -> EntryName {
    EntryName::new(format!("ledger-{id}"))
        .expect("invariant: ledger prefix and worktree identity form one ASCII entry")
}

pub(crate) fn frame(sequence: storage_contract::Sequence) -> EntryName {
    EntryName::new(format!("{:020}.json", sequence.get()))
        .expect("invariant: a bounded decimal sequence and JSON suffix form one entry")
}

pub(crate) fn edited_file(
    version: storage_contract::VersionId,
    path: &name_policy::PortablePath,
) -> EntryName {
    EntryName::new(format!(
        "{}-{}", storage_contract::VersionStorageId::from_version(version), path.file_name(),
    )).expect("invariant: a portable filename and fixed short identity form one entry")
}
