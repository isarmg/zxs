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

/// A pool configuration admitted at a file or public API boundary.
/// Only root rebinding and restart-only limits can change after admission.
///
/// ```compile_fail
/// use storage_engine::{Config, ValidatedConfig};
/// let mut config = ValidatedConfig::try_from(Config::development(["/disk".into()])).unwrap();
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
    workspace_limits: Option<super::RetainedWorkspaceLimits>,
}

impl TryFrom<Config> for ValidatedConfig {
    type Error = Error;

    fn try_from(config: Config) -> Result<Self> {
        validate_config(&config)?;
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
                workspace_limits: None,
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
    pub(super) fn same_admission(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.admitted, &other.admitted)
    }

    pub(super) fn verify_candidate_membership(
        &self,
        candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        let expected: BTreeSet<_> = self
            .metadata_members()
            .map(|disk| disk.disk_id.as_str())
            .collect();
        let actual: BTreeSet<_> = candidate
            .metadata_disk_ids
            .iter()
            .map(String::as_str)
            .collect();
        if expected.is_empty()
            || actual != expected
            || actual.len() != candidate.metadata_disk_ids.len()
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "candidate membership differs from configured metadata members",
            ));
        }
        Ok(())
    }
    pub(crate) fn verify_saved_metadata_membership(&self, saved: &Self) -> Result<()> {
        fn identity(member: &DiskConfig) -> (&str, &str, u64) {
            (&member.disk_id, &member.slot_id, member.binding_generation)
        }
        let expected: BTreeSet<_> = self.metadata_members().map(identity).collect();
        let actual: BTreeSet<_> = saved.metadata_members().map(identity).collect();
        if saved.pool_id != self.pool_id || actual != expected {
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

pub(super) enum DiskLockPolicy {
    NewPoolRoots,
    AvailableMembers,
}

pub(super) fn acquire_locks(
    root: &Path,
    config: &ValidatedConfig,
    policy: DiskLockPolicy,
) -> Result<Vec<File>> {
    let mut paths = vec![root.join("writer.lock")];
    for d in &config.disks {
        match policy {
            DiskLockPolicy::NewPoolRoots => {
                if !d.root.try_exists()? {
                    continue;
                }
            }
            DiskLockPolicy::AvailableMembers => {
                if disk_unavailable(d)? {
                    continue;
                }
                validate_seal(config, d)?;
            }
        }
        paths.push(d.root.join(".writer.lock"));
    }
    let mut locks = Vec::new();
    for path in paths {
        let f = File::from(
            rustix::fs::open(
                &path,
                rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::from_raw_mode(0o600),
            )
            .map_err(std::io::Error::from)?,
        );
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&f).map_err(|e| {
            let message = e.to_string();
            Error::from(e).context(storage_contract::ErrorCode::UnsafePath, message)
        })?;
        rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |source| {
                Error::from(std::io::Error::from(source)).context(
                    storage_contract::ErrorCode::WriterAlreadyActive,
                    "another process owns this pool or disk",
                )
            },
        )?;
        locks.push(f);
    }
    Ok(locks)
}
fn validate_config(c: &Config) -> Result<()> {
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
                || !successors.insert(up.to_ascii_lowercase())
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
