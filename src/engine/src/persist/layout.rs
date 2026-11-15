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

/// Descriptor/checksum/publication metadata is never a user namespace entry.
/// Every version_dir consumer has this one address; no old address is probed.
pub(crate) fn version_metadata(root: &Path) -> PathBuf {
    root.join(ZXS_DATA_ROOT.as_path())
}

pub(crate) static VERSIONS: LazyLock<EntryName> = LazyLock::new(|| {
    EntryName::new("versions").expect("invariant: versions uses one canonical entry")
});

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
