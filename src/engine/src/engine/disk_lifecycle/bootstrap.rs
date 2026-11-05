//! A durable restriction on discovery, never registration or membership authority.
//!
//! Engine startup loads/selects here before its first storage-root stat/open.
//! Discovery stays inside that selection and authenticates actual authority with
//! its original fixed N/Q. Any failure means management-only, not Config fallback.
//!
//! Initial roots must come from an already authenticated, held-resource session
//! or explicitly authorized retained-root recovery. Capturing a held descriptor
//! below observes its identity; it does not authenticate the supplied binding.
//! No API here grants writes, verifies publication, removes registrations, clears
//! a stop, or re-enables a root. Explicit recovery must use a separate authority
//! verifier. Directory identities do not imply independent physical devices.
mod format;
mod resume_observations;
pub(crate) mod runtime;
pub(crate) mod store;
use format::{Head, Record, RootRecord};
pub(crate) use runtime::RecoveredDiskStop;

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
impl std::fmt::Display for IntentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
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
    pub(in crate::engine) fn recorded_admission_epoch(
        &self,
        identity: &DiskRegistrationIdentity,
    ) -> Result<Option<String>> {
        let root = self
            .record
            .roots
            .iter()
            .find(|root| root.disk_id == identity.disk_id)
            .ok_or(BootstrapError::ManagementOnly(
                "admission epoch has no recorded binding",
            ))?;
        if identity.pool_id != self.record.pool_id
            || identity.slot_id != root.slot_id
            || identity.binding_generation != root.binding_generation
        {
            return Err(BootstrapError::ManagementOnly(
                "admission epoch names another binding",
            ));
        }
        Ok(self
            .record
            .admission_epoch(&identity.disk_id)
            .map(|value| value.to_string()))
    }
    pub(in crate::engine) fn verify_stopped_directory(
        &self,
        identity: &DiskRegistrationIdentity,
        path: &Path,
        physical: FileIdentity,
    ) -> Result<()> {
        let root = self
            .record
            .roots
            .iter()
            .find(|root| root.disk_id == identity.disk_id)
            .ok_or(BootstrapError::ManagementOnly(
                "readmission directory was not recorded",
            ))?;
        if self.record.pool_id != identity.pool_id
            || root.slot_id != identity.slot_id
            || root.binding_generation != identity.binding_generation
            || root.path != canonical_root(path)?
            || root.device != physical.device
            || root.inode != physical.inode
            || !self.record.is_stopped(&identity.disk_id)
        {
            return Err(BootstrapError::ManagementOnly(
                "readmission descriptor differs from the stopped recorded directory",
            ));
        }
        Ok(())
    }
    pub(in crate::engine) fn verify_readmissions(
        &self,
        authority: &crate::engine::membership_transition::pending::activation::PortableAuthority,
    ) -> crate::Result<()> {
        for admission in self.record.readmission_events() {
            let root = self.record.root(&admission.disk_id);
            authority.verify_readmission_hint(
                &DiskRegistrationIdentity {
                    pool_id: self.record.pool_id.clone(),
                    disk_id: root.disk_id.clone(),
                    slot_id: root.slot_id.clone(),
                    binding_generation: root.binding_generation,
                },
                admission.operation_id,
                &admission.intent_digest.to_string(),
                &admission.stopped_intent.to_string(),
            )?;
        }
        Ok(())
    }

    /// Pure alias/history validation. Only the typed terminal store methods may
    /// persist this change; this inspection never opens or admits a root.
    pub(in crate::engine) fn validate_terminal_root_expansion(
        &self,
        observations: Vec<RootObservation>,
    ) -> Result<()> {
        Record::resume_terminal(self, observations).map(|_| ())
    }

    fn permits_unstopped_relocation(&self, config: &Config) -> bool {
        self.record.stops.is_empty()
            && self.record.recovered_readmissions.is_empty()
            && config.disks.len() == self.record.roots.len()
            && config.disks.iter().all(|disk| {
                self.record
                    .roots
                    .iter()
                    .any(|root| root.disk_id == disk.disk_id)
            })
    }

    pub(crate) fn excludes_binding(&self, binding: &DiskRegistrationIdentity) -> bool {
        binding.pool_id == self.record.pool_id
            && self
                .record
                .roots
                .iter()
                .find(|root| root.disk_id == binding.disk_id)
                .is_none_or(|root| {
                    root.slot_id == binding.slot_id
                        && root.binding_generation == binding.binding_generation
                        && self.record.is_stopped(&binding.disk_id)
                })
    }

    /// Bind only recorded addresses after independent authority verification.
    /// This is a restriction of resources, not a change to registration or voters.
    pub(crate) fn restrict_config(
        &self,
        config: crate::ValidatedConfig,
        projected: &Config,
    ) -> crate::Result<crate::ValidatedConfig> {
        let permits_relocation = self.permits_unstopped_relocation(projected);
        let mut allowed = std::collections::BTreeSet::new();
        let mut roots = Vec::with_capacity(config.disks.len());
        for disk in &config.disks {
            let observed = self
                .record
                .roots
                .iter()
                .find(|root| root.disk_id == disk.disk_id);
            if let Some(root) = observed {
                if root.slot_id != disk.slot_id
                    || root.binding_generation != disk.binding_generation
                {
                    return Err(crate::Error::new(
                        crate::ErrorCode::RecoveryRequired,
                        "bootstrap binding differs from independently authenticated configuration",
                    ));
                }
                if !self.record.is_stopped(&disk.disk_id)
                    && self
                        .record
                        .admission_epoch(&disk.disk_id)
                        .map(|value| value.to_string())
                        == config.io_admission_epoch(&disk.disk_id)?
                {
                    allowed.insert(disk.disk_id.clone());
                }
                let projected_root = permits_relocation.then(|| {
                    projected
                        .disks
                        .iter()
                        .find(|disk| disk.disk_id == root.disk_id)
                        .expect("invariant: complete locator projection")
                        .root
                        .clone()
                });
                roots.push(projected_root.unwrap_or_else(|| PathBuf::from(&root.path)));
            } else {
                roots.push(disk.root.clone());
            }
        }
        config.with_disk_roots(roots)?.restrict_io_disks(allowed)
    }

    #[cfg(test)]
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
                admission_epoch: stop.admission_epoch,
            }
        })
    }
    /// Pure selection. No filesystem operation, including canonicalize/stat,
    /// occurs here. Stale Config may retain stopped roots but cannot widen the
    /// selection or remap any recorded binding. Before the first stop only, a
    /// complete projection may relocate the same observed directory identity.
    /// Once restricted, changed addresses require explicit retained-root recovery.
    #[cfg(test)]
    pub(crate) fn select<'a>(&'a self, config: &'a Config) -> Result<RestrictedProbes<'a>> {
        self.select_readmission_probes(config, &[])
    }

    pub(in crate::engine) fn select_readmission_probes<'a>(
        &'a self,
        config: &'a Config,
        admissions: &[crate::engine::membership_transition::readmission::ReadmissionProbeAdmission],
    ) -> Result<RestrictedProbes<'a>> {
        if self.record.is_terminal() {
            return Err(BootstrapError::ManagementOnly(
                "terminal locator permits no storage-root discovery; authenticate protected management authority",
            ));
        }
        if config.pool_id != self.head.pool_id || config.disks.len() > MAX_ROOTS {
            return Err(BootstrapError::ManagementOnly(
                "projection pool or size differs",
            ));
        }
        let permits_relocation = self.permits_unstopped_relocation(config);
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
            let Some(known) = self
                .record
                .roots
                .iter()
                .find(|root| root.disk_id == disk.disk_id)
            else {
                // Explicit retained-root recovery can record fewer addresses
                // than the full historical configuration. Unknown addresses
                // remain forbidden; Config never expands the probe set.
                continue;
            };
            if (known.path != path && !permits_relocation)
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
            let root = self.record.root(disk_id);
            if let Some(epoch) = self.record.admission_epoch(disk_id) {
                let event = self
                    .record
                    .readmission_events()
                    .find(|event| event.disk_id == *disk_id && event.intent_digest == epoch)
                    .expect("invariant: ledger admission epoch exists");
                let identity = DiskRegistrationIdentity {
                    pool_id: self.record.pool_id.clone(),
                    disk_id: root.disk_id.clone(),
                    slot_id: root.slot_id.clone(),
                    binding_generation: root.binding_generation,
                };
                if !admissions.iter().any(|proof| {
                    proof.admits(
                        &identity,
                        Path::new(&root.path),
                        FileIdentity {
                            device: root.device,
                            inode: root.inode,
                        },
                        event.operation_id,
                        &epoch.to_string(),
                        &event.stopped_intent.to_string(),
                    )
                }) {
                    // An event alone never widens discovery. Callers without a
                    // certified exact probe scope retain the safe restricted set.
                    continue;
                }
            }
            let path = if permits_relocation {
                config
                    .disks
                    .iter()
                    .find(|disk| disk.disk_id == *disk_id)
                    .expect("invariant: complete locator projection")
                    .root
                    .as_path()
            } else {
                Path::new(&root.path)
            };
            roots.push(ReadProbe {
                root,
                pool_id: &self.record.pool_id,
                path,
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
    pub(crate) admission_epoch: Option<IntentDigest>,
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
    path: &'a Path,
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
        self.path
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
