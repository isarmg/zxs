//! A durable restriction on discovery, never registration or membership authority.
//!
//! This is NOT wired into Engine::open or membership discovery yet. The caller
//! must load/select here before its first storage-root stat/open, keep subsequent
//! discovery inside that selection, and authenticate actual authority with its
//! original fixed N/Q. Any failure means management-only, not Config fallback.
//!
//! Initial roots must come from an already authenticated, held-resource session
//! or explicitly authorized retained-root recovery. Capturing a held descriptor
//! below observes its identity; it does not authenticate the supplied binding.
//! No API here grants writes, verifies publication, removes registrations, clears
//! a stop, or re-enables a root. Explicit recovery must use a separate authority
//! verifier. Directory identities do not imply independent physical devices.
mod format;
pub(crate) mod store;
use format::{Head, Record, RootRecord};

use crate::{Config, OperationId, engine::disk_registry::DiskRegistrationIdentity};
use platform_io::fs_safety::linux::FileIdentity;
use std::{
    fs::File,
    path::{Component, Path, PathBuf},
};

const MAX_RECORD_BYTES: usize = 128 * 1024;
// Every serialized root needs more than 128 bytes. This is an input-allocation
// bound derived from the object budget, not a configured pool-size policy.
const MAX_ROOTS: usize = MAX_RECORD_BYTES / 128;
const MAX_PATH_BYTES: usize = 4096;

type Result<T> = std::result::Result<T, BootstrapError>;

/// Every failure is a closed discovery gate. Preserve the concrete cause for
/// management recovery; never translate NotFound into permission to use Config.
#[derive(Debug, error_definition::Error)]
pub(crate) enum BootstrapError {
    #[error("bootstrap {stage} filesystem failure: {source}")]
    Filesystem {
        stage: &'static str,
        #[source]
        source: platform_io::fs_safety::Error,
    },
    #[error("bootstrap {stage} I/O failure: {source}")]
    Io {
        stage: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("bootstrap {stage} encoding failure: {source}")]
    Encoding {
        stage: &'static str,
        #[source]
        source: storage_contract::Error,
    },
    #[error("bootstrap slot identity is invalid: {source}")]
    Slot {
        #[source]
        source: name_policy::Error,
    },
    #[error("bootstrap intent digest is invalid: {source}")]
    Digest {
        #[source]
        source: storage_contract::ParseContentHashError,
    },
    #[error("bootstrap atomic publication failed: {source}")]
    Commit {
        #[source]
        source: platform_io::fs_safety::AtomicCommitError,
    },
    #[error("bootstrap requires management recovery: {0}")]
    ManagementOnly(&'static str),
}
impl BootstrapError {
    fn fs(stage: &'static str, source: platform_io::fs_safety::Error) -> Self {
        Self::Filesystem { stage, source }
    }
    fn io(stage: &'static str, source: std::io::Error) -> Self {
        Self::Io { stage, source }
    }
    fn encoding(stage: &'static str, source: storage_contract::Error) -> Self {
        Self::Encoding { stage, source }
    }
}

/// Observed held-directory metadata, explicitly NOT a verified registration.
#[derive(Clone, Debug)]
pub(crate) struct RootObservation {
    pool_id: String,
    record: RootRecord,
}
impl RootObservation {
    pub(crate) fn capture(
        binding: &DiskRegistrationIdentity,
        root: &Path,
        directory: &File,
        metadata_member: bool,
    ) -> Result<Self> {
        let path = canonical_root(root)?;
        let metadata = directory
            .metadata()
            .map_err(|e| BootstrapError::io("held root identity", e))?;
        if !metadata.is_dir() {
            return Err(BootstrapError::ManagementOnly(
                "held root is not a directory",
            ));
        }
        let identity = FileIdentity::from_metadata(&metadata);
        let record = RootRecord {
            disk_id: binding.disk_id.clone(),
            slot_id: binding.slot_id.clone(),
            binding_generation: binding.binding_generation,
            path,
            device: identity.device,
            inode: identity.inode,
            metadata_member,
        };
        record.validate()?;
        storage_contract::validate_id(&binding.pool_id, "pool")
            .map_err(|e| BootstrapError::encoding("pool identity", e))?;
        Ok(Self {
            pool_id: binding.pool_id.clone(),
            record,
        })
    }
}

/// Digest syntax only. It asserts neither intent existence nor publication.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    data_serialization::Serialize,
    data_serialization::Deserialize,
)]
#[serialization(transparent)]
pub(crate) struct IntentDigest(storage_contract::ContentHash);
impl IntentDigest {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        value
            .parse()
            .map(Self)
            .map_err(|source| BootstrapError::Digest { source })
    }
}

/// A canonical, internally consistent local ledger snapshot. It is still only
/// a stop hint. Real authority must be authenticated after restricted probing.
#[derive(Clone, Debug)]
pub(crate) struct LoadedLocator {
    head: Head,
    record: Record,
}
impl LoadedLocator {
    pub(crate) fn generation(&self) -> u64 {
        self.head.generation
    }

    /// Pending restrictions to reconcile against a REAL intent/authority
    /// verifier. Absence of an intent is not permission to remove its stop.
    pub(crate) fn stop_hints(&self) -> impl Iterator<Item = StopHint<'_>> {
        self.record.stops.iter().map(|stop| {
            let root = self.record.root(&stop.disk_id);
            StopHint {
                pool_id: &self.record.pool_id,
                disk_id: &root.disk_id,
                slot_id: &root.slot_id,
                binding_generation: root.binding_generation,
                operation_id: stop.operation_id,
                intent_digest: stop.intent_digest,
            }
        })
    }
    /// Pure selection. No filesystem operation, including canonicalize/stat,
    /// occurs here. Stale Config may retain stopped roots but cannot widen the
    /// selection, remap any recorded binding, or introduce an unknown address.
    pub(crate) fn select<'a>(&'a self, config: &Config) -> Result<RestrictedProbes<'a>> {
        if config.pool_id != self.head.pool_id || config.disks.len() > MAX_ROOTS {
            return Err(BootstrapError::ManagementOnly(
                "projection pool or size differs",
            ));
        }
        let mut seen_ids = std::collections::BTreeSet::new();
        let mut seen_paths = Vec::new();
        for disk in &config.disks {
            let path = canonical_root(&disk.root)?;
            if !seen_ids.insert(disk.disk_id.clone())
                || seen_paths.iter().any(|old: &String| overlaps(old, &path))
            {
                return Err(BootstrapError::ManagementOnly(
                    "projection contains duplicate or overlapping roots",
                ));
            }
            seen_paths.push(path.clone());
            let known = self
                .record
                .roots
                .iter()
                .find(|root| root.disk_id == disk.disk_id)
                .ok_or(BootstrapError::ManagementOnly(
                    "projection has an unknown root or alias",
                ))?;
            if known.path != path
                || known.slot_id != disk.slot_id
                || known.binding_generation != disk.binding_generation
            {
                return Err(BootstrapError::ManagementOnly(
                    "projection root binding or address changed",
                ));
            }
        }
        let mut roots = Vec::new();
        for disk_id in &self.record.retained {
            if !seen_ids.contains(disk_id) {
                return Err(BootstrapError::ManagementOnly(
                    "projection lacks a retained metadata root",
                ));
            }
            let root = self.record.root(disk_id);
            roots.push(ReadProbe {
                root,
                pool_id: &self.record.pool_id,
            });
        }
        Ok(RestrictedProbes { roots })
    }
}

/// Unauthenticated local restriction metadata, not an operation outcome.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StopHint<'a> {
    pub(crate) pool_id: &'a str,
    pub(crate) disk_id: &'a str,
    pub(crate) slot_id: &'a str,
    pub(crate) binding_generation: u64,
    pub(crate) operation_id: OperationId,
    pub(crate) intent_digest: IntentDigest,
}

/// Only addresses permitted for read-only authority discovery, NOT trusted
/// authority. No Config is returned and no quorum or membership is calculated.
/// Subsequent seal/fence hints must not expand this set. After opening a retained
/// root, its held identity must match the recorded observation before reading
/// metadata; unknown path aliases/rebindings cannot be proved absent by strings.
/// When their safety is uncertain, do not call the probe: use management recovery.
pub(crate) struct RestrictedProbes<'a> {
    roots: Vec<ReadProbe<'a>>,
}
impl<'a> RestrictedProbes<'a> {
    /// The callback is the first permitted storage I/O boundary. In particular,
    /// never preflight all Config roots before invoking this restricted loop.
    pub(crate) fn try_for_each<E>(
        &self,
        mut probe: impl FnMut(ReadProbe<'a>) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        for root in &self.roots {
            probe(*root)?;
        }
        Ok(())
    }
    pub(crate) fn len(&self) -> usize {
        self.roots.len()
    }
}
#[derive(Clone, Copy)]
pub(crate) struct ReadProbe<'a> {
    root: &'a RootRecord,
    pool_id: &'a str,
}
impl ReadProbe<'_> {
    pub(crate) fn binding(&self) -> DiskRegistrationIdentity {
        DiskRegistrationIdentity {
            pool_id: self.pool_id.into(),
            disk_id: self.root.disk_id.clone(),
            slot_id: self.root.slot_id.clone(),
            binding_generation: self.root.binding_generation,
        }
    }
    pub(crate) fn path(&self) -> &Path {
        Path::new(&self.root.path)
    }
    pub(crate) fn disk_id(&self) -> &str {
        &self.root.disk_id
    }
    pub(crate) fn expected_identity(&self) -> FileIdentity {
        FileIdentity {
            device: self.root.device,
            inode: self.root.inode,
        }
    }
}

fn canonical_root(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or(BootstrapError::ManagementOnly("root is not UTF-8"))?;
    let rebuilt: PathBuf = path.components().collect();
    if !path.is_absolute()
        || path.parent().is_none()
        || value.len() > MAX_PATH_BYTES
        || value.as_bytes().contains(&0)
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        || rebuilt.as_os_str() != path.as_os_str()
    {
        return Err(BootstrapError::ManagementOnly(
            "root path is not bounded canonical absolute syntax",
        ));
    }
    Ok(value.to_owned())
}
fn overlaps(a: &str, b: &str) -> bool {
    Path::new(a).starts_with(b) || Path::new(b).starts_with(a)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/bootstrap_locator_tests.rs"]
mod tests;
