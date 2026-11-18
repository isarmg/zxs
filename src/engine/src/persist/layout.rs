//! Stable entry names shared by runtime publication and recovery.
pub(crate) mod worktree;
pub(crate) mod container;
pub(crate) mod namespace;
use platform_io::fs_safety::{EntryName, RelativePath};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub(crate) mod snapshot;

pub(crate) static SYSTEM_CONFIGURATION: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("config.json").expect("invariant: system configuration is one entry")
});

/// Physical roots of the unified working-tree layout. These names do not by
/// themselves admit a directory or authorize adoption of existing material.
pub(crate) static COMMON_DATA_ROOT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("common-data").expect("invariant: common data root is one canonical entry")
});

pub(crate) static DELETED_DATA_ROOT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("deleted-data").expect("invariant: deleted data root is one canonical entry")
});

pub(crate) static ZXS_DATA_ROOT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("zxs-data").expect("invariant: management data root is one canonical entry")
});

/// The sole persisted fixed-genesis configuration address in this layout.
/// It is written after physical root capture and consumed by normal cold admission.
pub(crate) static GENESIS_CONFIGURATION: LazyLock<RelativePath> = LazyLock::new(|| {
    RelativePath::new("zxs-data/pool-membership/config.json")
        .expect("invariant: genesis configuration has one canonical management address")
});

/// Only an admitted repository binding can classify this entry as management data.
pub(crate) static SNAPSHOT_REPOSITORY: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new(".zxs").expect("invariant: snapshot repository is one canonical entry")
});

/// Address ordinary current material in the one supported fresh layout.
/// Root admission compares its physical binding before runtime resources open.
/// This pure address helper never falls back to the retired histories tree.
pub(crate) fn histories(root: &Path) -> PathBuf {
    root.join(COMMON_DATA_ROOT.as_path())
}

/// The legacy raw locator does not supply a source/lifecycle admission.
/// Ordinary live/cold version material uses `source_version` below an admitted
/// Current lifecycle FD. Private native packages retain their own codec.
pub(crate) fn version_metadata(root: &Path) -> PathBuf {
    root.join(ZXS_DATA_ROOT.as_path())
}

pub(crate) static VERSIONS: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("versions").expect("invariant: versions uses one canonical entry")
});

pub(crate) static VERSION_DESCRIPTOR: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("version.json").expect("invariant: version descriptor is one entry")
});

pub(crate) static VERSION_CHECKSUMS: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("checksums.bin").expect("invariant: checksum manifest is one entry")
});

pub(crate) static VERSION_PAYLOAD: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("payload.json").expect("invariant: payload descriptor is one entry")
});

pub(crate) static VERSION_PREPARATIONS: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new(".zxs-prepared").expect("invariant: version preparation is one entry")
});

pub(crate) static PREPARED_CONTENT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("content").expect("invariant: prepared content is one entry")
});

/// Relative to an already admitted source lifecycle root, never a disk root.
pub(crate) fn source_version(
    history: storage_contract::HistoryTreeId,
    version: storage_contract::VersionId,
) -> PathBuf {
    PathBuf::from(history.to_string()).join(VERSIONS.as_path()).join(version.to_string())
}

pub(crate) fn source_version_preparation(
    history: storage_contract::HistoryTreeId,
    operation: storage_contract::OperationId,
) -> PathBuf {
    PathBuf::from(history.to_string())
        .join(VERSION_PREPARATIONS.as_path()).join(operation.to_string())
}

pub(crate) static VERSION_PUBLICATION_PROOF: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("publication-proof.json")
        .expect("invariant: local publication proof uses one canonical entry")
});

pub(crate) static VERSION_COMMIT_PROOF: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("commit-proof.json")
        .expect("invariant: local commit proof uses one canonical entry")
});

pub(crate) static VERSION_TOMBSTONE: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("purged.json").expect("invariant: local tombstone uses one canonical entry")
});

pub(crate) static DISK_IDENTITY: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("disk.json").expect("invariant: disk identity uses one canonical entry")
});

/// Existing helper material name, observed only under the original selected lower parent.
pub(crate) static FOREIGN_CHAIN_CAPSULE: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("foreign-chain-material").expect("invariant: Foreign capsule has one entry name")
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

/// Address below a held disk root, shared by publication, loading and coverage exclusion.
pub(crate) static REPLAY_CHECKPOINT: LazyLock<RelativePath> = LazyLock::new(|| {
    RelativePath::new("metadata/replay-checkpoint.json")
        .expect("invariant: replay checkpoint uses a canonical disk-relative path")
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

/// Inode-bound decided rename intent; retained for exact crash recovery.
pub(crate) static VERSION_NAMES_INTENT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new(".zxs-version-names.json")
        .expect("invariant: version rename intent uses one canonical entry")
});

/// Private Add feedback stays on original members, not the unenrolled target.
pub(crate) static MEMBERSHIP_ROOT_PREPARATIONS: LazyLock<RelativePath> = LazyLock::new(|| {
    RelativePath::new("metadata/membership-fresh-root-preparations")
        .expect("invariant: root preparation feedback has one disk-relative address")
});

pub(crate) fn membership_root_preparation(operation: storage_contract::OperationId) -> EntryName {
    EntryName::new(format!("{operation}.json"))
        .expect("invariant: a typed operation forms one preparation entry")
}

pub(crate) fn membership_layout_stage(
    id: storage_contract::working_tree::DirectoryId,
) -> EntryName {
    EntryName::new(format!("membership-layout-{id}"))
        .expect("invariant: a typed DirectoryId forms one private preparation entry")
}

/// Backup target feedback is retained by every original fenced metadata member.
pub(crate) static BACKUP_ROOT_PREPARATIONS: LazyLock<RelativePath> = LazyLock::new(|| {
    RelativePath::new("metadata/backup-root-preparations")
        .expect("invariant: Backup root feedback has one disk-relative address")
});

pub(crate) fn backup_root_preparation(operation: storage_contract::OperationId) -> EntryName {
    EntryName::new(format!("{operation}.json"))
        .expect("invariant: a typed operation forms one Backup preparation entry")
}

/// The complete prefix/parent/FILE feedback shares the original fixed fence.
pub(crate) static BACKUP_LAYOUT_PREPARATIONS: LazyLock<RelativePath> = LazyLock::new(|| {
    RelativePath::new("metadata/backup-layout-preparations")
        .expect("invariant: Backup layout feedback has one disk-relative address")
});

/// The retained native Backup package is not the ordinary source namespace.
/// Its consumer must already hold the selected package admission.
pub(crate) fn backup_package_version(
    history: storage_contract::HistoryTreeId, version: storage_contract::VersionId,
) -> PathBuf {
    PathBuf::from("histories").join(history.to_string())
        .join(VERSIONS.as_path()).join(version.to_string())
}


/// Global native Backup metadata remains outside every source lifecycle slice.
pub(crate) static BACKUP_NATIVE_METADATA: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("metadata").expect("invariant: native Backup metadata is one entry")
});


/// Inert original Foreign intent. No discovery, startup or selection follows
/// an entry merely because this name exists on SYSTEM.
pub(crate) static FOREIGN_ENROLLMENT: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("foreign-enrollment").expect("invariant: Foreign provenance is one entry")
});

pub(crate) fn foreign_enrollment(operation: storage_contract::OperationId) -> EntryName {
    EntryName::new(format!("{operation}.json"))
        .expect("invariant: typed operation addresses exactly one Foreign original")
}
