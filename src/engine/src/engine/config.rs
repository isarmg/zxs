//! Disk topology, resource admission configuration and OS writer locks.
use crate::{disk::*, *};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

mod membership;
use membership::MetadataMembership;
mod io_authority;
use io_authority::GenesisIoAuthority;

/// A pool configuration admitted at a file or public API boundary.
/// Only root rebinding and restart-only limits can change after admission.
///
/// ```compile_fail
/// use storage_engine::{Config, ValidatedConfig};
/// let mut config = ValidatedConfig::try_from(Config::development(["/disk".into()]).unwrap()).unwrap();
/// config.disks.clear();
/// ```
#[derive(Clone, Debug, Deserialize)]
#[serialization(try_from = "Config")]
pub struct ValidatedConfig {
    admitted: Arc<AdmittedConfig>,
}

#[derive(Clone, Debug)]
struct AdmittedConfig {
    config: Config,
    metadata_members: Box<[usize]>,
    membership: MetadataMembership,
    genesis_io_authority: Arc<GenesisIoAuthority>,
    workspace_limits: Option<super::RetainedWorkspaceLimits>,
    generation: Option<Arc<super::membership_transition::pending::activation::Generation>>,
    // Admission-only restriction; never serialized or used to calculate N/Q.
    io_disks: Option<BTreeSet<String>>,
}

impl TryFrom<Config> for ValidatedConfig {
    type Error = Error;

    fn try_from(config: Config) -> Result<Self> {
        validate_config(&config)?;
        let membership = MetadataMembership::from_config(&config);
        let genesis_io_authority = Arc::new(GenesisIoAuthority::from_config(&config));
        let metadata_members = config
            .disks
            .iter()
            .enumerate()
            .filter_map(|(index, disk)| disk.metadata_member.then_some(index))
            .collect();
        Ok(Self {
            admitted: Arc::new(AdmittedConfig {
                config,
                metadata_members,
                membership,
                genesis_io_authority,
                workspace_limits: None,
                generation: None,
                io_disks: None,
            }),
        })
    }
}

impl Serialize for ValidatedConfig {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.admitted.config.serialize(serializer)
    }
}

impl Deref for ValidatedConfig {
    type Target = Config;

    fn deref(&self) -> &Config {
        &self.admitted.config
    }
}

impl ValidatedConfig {
    /// A deterministic configuration projection of the exact admitted Backup
    /// choice, not a Genesis admission of the new targets. Only the native
    /// Backup verifier supplies this closed projection. Original I/O and voter
    /// authority are retained; no newly configured root receives an I/O lease.
    pub(in crate::engine) fn for_backup_projection(
        original: &Self,
        projection: &super::backup_preparation::BackupRegistryProjection,
    ) -> Result<Self> {
        if original.generation().is_some() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "Backup configuration projection requires its fixed original Genesis",
            ));
        }
        let config = projection.configuration(original)?;
        validate_config_shape(&config, true)?;
        let mut result = original.clone();
        let admitted = Arc::make_mut(&mut result.admitted);
        admitted.config = config;
        // The original prefix and its immutable metadata indexes are unchanged.
        // Never create a fresh GenesisIoAuthority from these selected roots.
        admitted.io_disks = Some(
            original
                .io_disks()
                .map(|disk| disk.disk_id.clone())
                .collect(),
        );
        Ok(result)
    }

    /// The latest authenticated readmission defines only this directory's I/O
    /// lifetime. It is never part of Config serialization or historical binding.
    pub(crate) fn io_admission_epoch(&self, disk_id: &str) -> Result<Option<String>> {
        self.generation()
            .map(|generation| generation.portable.io_admission_epoch(disk_id))
            .transpose()
            .map(Option::flatten)
    }

    /// Actual startup resources are a subset of the authenticated configuration.
    /// Historical registration and fixed voter denominators remain unchanged.
    pub(crate) fn io_disks(&self) -> impl Iterator<Item = &DiskConfig> {
        self.disks.iter().filter(|disk| {
            self.admitted
                .io_disks
                .as_ref()
                .is_none_or(|ids| ids.contains(&disk.disk_id))
        })
    }

    pub(crate) fn retired_io_epochs(
        &self,
        disk_id: &str,
    ) -> Result<std::collections::BTreeSet<Option<String>>> {
        self.generation()
            .map(|generation| generation.portable.retired_io_epochs(disk_id))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub(crate) fn excludes_io_disk(&self, disk_id: &str) -> bool {
        self.admitted
            .io_disks
            .as_ref()
            .is_some_and(|ids| !ids.contains(disk_id))
    }

    pub(crate) fn restrict_io_disks(mut self, ids: BTreeSet<String>) -> Result<Self> {
        if ids
            .iter()
            .any(|id| !self.io_disks().any(|disk| &disk.disk_id == id))
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "I/O admission cannot add or restore a configured disk",
            ));
        }
        Arc::make_mut(&mut self.admitted).io_disks = Some(ids);
        Ok(self)
    }

    pub(crate) fn same_admission(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.admitted, &other.admitted)
    }

    /// Context equality only. Authentication still belongs to Genesis admission
    /// or the verified successor; neither schema validity nor equal voters suffice.
    pub(crate) fn same_genesis_io_authority(&self, other: &Self) -> bool {
        Arc::ptr_eq(
            &self.admitted.genesis_io_authority,
            &other.admitted.genesis_io_authority,
        ) || self.admitted.genesis_io_authority == other.admitted.genesis_io_authority
    }

    /// The fixed saved Genesis authority permits only tighter local retained
    /// limits. This is a recovery projection check, not I/O-context equality.
    pub(crate) fn permits_genesis_recovery_projection(&self, projected: &Self) -> bool {
        self.admitted
            .genesis_io_authority
            .permits_recovery_projection(&projected.admitted.genesis_io_authority)
    }

    pub(crate) fn verify_candidate_membership(
        &self,
        candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        match &self.admitted.generation {
            Some(generation) => generation.verify_candidate(candidate),
            None if candidate.generation().is_none() => {
                self.admitted.membership.verify_candidate(candidate)
            }
            None => Err(Error::new(
                ErrorCode::PublicationInvalid,
                "generation candidate has no admitted authority",
            )),
        }
    }

    pub(crate) fn verify_original_candidate_membership(
        &self,
        candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        self.admitted.membership.verify_candidate(candidate)
    }
    pub(crate) fn generation(
        &self,
    ) -> Option<&Arc<super::membership_transition::pending::activation::Generation>> {
        self.admitted.generation.as_ref()
    }
    pub(crate) fn generation_hash(&self) -> Option<&str> {
        self.generation().map(|generation| generation.hash.as_str())
    }
    pub(crate) fn membership_epoch(&self) -> u64 {
        match self.generation() {
            None => 1,
            Some(generation) => generation.membership_epoch(),
        }
    }
    pub(crate) fn with_generation(
        mut self,
        generation: Arc<super::membership_transition::pending::activation::Generation>,
    ) -> Result<Self> {
        if self.admitted.generation.is_some() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "generation is already admitted",
            ));
        }
        Arc::make_mut(&mut self.admitted).generation = Some(generation);
        Ok(self)
    }

    pub(crate) fn same_metadata_membership(&self, other: &Self) -> bool {
        self.admitted.membership == other.admitted.membership
    }

    pub(crate) fn verify_saved_metadata_membership(&self, saved: &Self) -> Result<()> {
        if !self.same_metadata_membership(saved) {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "saved metadata membership differs from admitted configuration",
            ));
        }
        Ok(())
    }

    pub(super) fn metadata_members(&self) -> impl ExactSizeIterator<Item = &DiskConfig> {
        self.admitted
            .metadata_members
            .iter()
            .map(|&index| &self.admitted.config.disks[index])
    }

    /// Rebind the existing disks in order, preserving membership and resource limits.
    ///
    /// # Errors
    /// Rejects a different disk count, relative roots, traversal or overlapping roots.
    pub fn with_disk_roots(mut self, roots: Vec<PathBuf>) -> Result<Self> {
        if roots.len() != self.admitted.config.disks.len() {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "disk root count differs from configured membership",
            ));
        }
        validate_disk_roots(roots.iter().map(PathBuf::as_path))?;
        for (disk, root) in Arc::make_mut(&mut self.admitted)
            .config
            .disks
            .iter_mut()
            .zip(roots)
        {
            disk.root = root;
        }
        Ok(self)
    }

    /// Bind restart-only overrides before any initialization intent is published.
    ///
    /// # Errors
    /// Rejects zero limits and overrides below the persisted pool baseline.
    pub fn with_workspace_limits(
        mut self,
        limits: Option<super::RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        if let Some(limits) = limits {
            limits.validate_for(&self)?;
        }
        Arc::make_mut(&mut self.admitted).workspace_limits = limits;
        Ok(self)
    }

    pub(super) fn effective_workspace_limits(&self) -> super::RetainedWorkspaceLimits {
        self.admitted
            .workspace_limits
            .unwrap_or_else(|| super::RetainedWorkspaceLimits::from_config(self))
    }
}

/// Admit one recovery input against its own anchor. Selection still requires a
/// configured majority and later checks every input seal against that winner.
pub(super) fn read_recovery_anchor(path: &Path, seal: &DiskSeal) -> Result<ValidatedConfig> {
    let original = ValidatedConfig::try_from(read_json::<Config>(
        &path.join("metadata/pool-membership/config.json"),
    )?)?;
    let config =
        super::membership_transition::pending::activation::restore_anchor(path, seal, original)?;
    let mut member = config
        .disks
        .iter()
        .find(|disk| disk.disk_id == seal.disk_id && disk.metadata_member)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "recovery input is not a metadata member of its saved configuration",
            )
        })?;
    member.root = path.to_path_buf();
    validate_seal(&config, &member)?;
    Ok(config)
}

pub(super) fn acquire_locks(root: &Path, disks: &DiskResources) -> Result<WriterLocks> {
    acquire_locks_selected(
        root,
        disks,
        crate::authority_selection::SelectionGuard::legacy(root)?,
    )
}

pub(super) fn acquire_locks_selected(
    root: &Path,
    disks: &DiskResources,
    selection: crate::authority_selection::SelectionGuard,
) -> Result<WriterLocks> {
    selection.require_legacy()?;
    let system = File::from(
        rustix::fs::open(
            root.join("writer.lock"),
            rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::RDWR
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?,
    );
    acquire_writer(&system)?;
    selection.require_legacy()?;
    let mut locks = WriterLocks::new(system, selection);
    for disk in disks.handles() {
        let file = disk.root.lock_file()?;
        acquire_writer(&file)?;
        let owner = FenceOwner::new(file, None);
        locks.insert(
            disk.seal.disk_id.clone(),
            DiskWriter::adopt(&disk.root, owner)?,
        );
    }
    Ok(locks)
}

fn acquire_writer(f: &File) -> Result<()> {
    platform_io::fs_safety::linux::SingleLinkRequirement::check(f).map_err(|e| {
        let message = e.to_string();
        Error::from(e).context(storage_contract::ErrorCode::UnsafePath, message)
    })?;
    rustix::fs::flock(f, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
        |source| {
            Error::from(std::io::Error::from(source)).context(
                storage_contract::ErrorCode::WriterAlreadyActive,
                "another process owns this pool or disk",
            )
        },
    )?;
    Ok(())
}
fn validate_config(c: &Config) -> Result<()> {
    validate_config_shape(c, false)
}

// Fanout is admitted only through the exact native Backup projection above.
// Public Config/TryFrom/initialization keeps its legacy linear restriction.
fn validate_config_shape(c: &Config, native_backup: bool) -> Result<()> {
    validate_id(&c.pool_id, "pool")?;
    if !c.disks.iter().any(|d| d.metadata_member)
        || c.max_file_bytes == 0
        || c.max_workspace_bytes == 0
        || c.max_file_bytes > c.max_workspace_bytes
        || c.max_workspaces == 0
        || c.max_retained_workspaces == 0
        || c.max_retained_workspaces_per_principal == 0
        || c.max_retained_workspace_metadata_bytes == 0
        || c.max_retained_workspace_metadata_bytes_per_principal == 0
        || c.max_entries == 0
        || !SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&c.checksum_chunk_bytes)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::ConfigInvalid,
            "invalid metadata membership, resource limits or checksum size",
        ));
    }
    let chunks = c.max_file_bytes.div_ceil(c.checksum_chunk_bytes as u64);
    if chunks > (MAX_CHECKSUM_MANIFEST as u64 - CHECKSUM_HEADER_BYTES as u64) / 32 {
        return Err(Error::new(
            storage_contract::ErrorCode::ConfigInvalid,
            "file limit exceeds the bounded checksum manifest capacity",
        ));
    }
    let mut slots = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut successors = BTreeSet::new();
    for d in &c.disks {
        name_policy::validate_slot(&d.slot_id)?;
        validate_id(&d.disk_id, "disk")?;
        if !slots.insert(d.slot_id.to_ascii_lowercase())
            || !ids.insert(&d.disk_id)
            || d.level == 0
            || d.binding_generation == 0
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ConfigInvalid,
                "duplicate disk, path or slot identity",
            ));
        }
        if d.level == 1 && d.upstream_slot_id.is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::TopologyInvalid,
                "primary cannot have an upstream",
            ));
        }
        if d.level > 1 {
            let up = d.upstream_slot_id.as_ref().ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::TopologyInvalid,
                    "backup lacks upstream",
                )
            })?;
            let source = c
                .disks
                .iter()
                .find(|s| s.slot_id.eq_ignore_ascii_case(up))
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::TopologyInvalid,
                        "upstream slot missing",
                    )
                })?;
            if source.level.checked_add(1) != Some(d.level)
                || (!native_backup && !successors.insert(up.to_ascii_lowercase()))
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::TopologyInvalid,
                    "route skips a level or source has multiple successors",
                ));
            }
        }
    }
    validate_disk_roots(c.disks.iter().map(|disk| disk.root.as_path()))
}

fn validate_disk_roots<'a>(roots: impl IntoIterator<Item = &'a Path>) -> Result<()> {
    let mut checked: Vec<&Path> = Vec::new();
    for root in roots {
        if !root.is_absolute()
            || root
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "disk roots must be absolute and contain no parent traversal",
            ));
        }
        if checked
            .iter()
            .any(|other| root.starts_with(other) || other.starts_with(root))
        {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "disk roots must not overlap",
            ));
        }
        checked.push(root);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/config/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/config/membership_recovery_tests.rs"]
mod membership_recovery_tests;
