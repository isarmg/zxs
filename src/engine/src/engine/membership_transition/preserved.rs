//! The complete Foreign action has a separate, read-only proof path. Ordinary
//! Add's root reservations and fence cannot express preserved source identity.
use super::*;
use crate::foreign_import::{LockedCapsuleParent, PreservedSourceInventory};
use crate::persist::layout;
use platform_io::fs_safety::linux::{AdvisoryLock, FileIdentity};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, error_definition::Error)]
#[error("Foreign provenance is not a membership decision; the current pool was not fenced")]
pub(super) struct NotAdmitted;
impl From<NotAdmitted> for Error {
    fn from(failure: NotAdmitted) -> Self {
        storage_contract::Error::new(ErrorCode::UnsupportedOperation, failure.to_string()).into()
    }
}

#[derive(Debug, error_definition::Error)]
enum Invalid {
    #[error("preserved Foreign chain exceeds its complete root budget")]
    Count,
    #[error("original Foreign registration, material or named physical root changed")]
    Binding,
}
impl From<Invalid> for Error {
    fn from(failure: Invalid) -> Self {
        let code = match failure {
            Invalid::Count => ErrorCode::ResourceLimit,
            Invalid::Binding => ErrorCode::DiskIdentityMismatch,
        };
        storage_contract::Error::new(code, failure.to_string()).into()
    }
}

fn require_count(current: usize, added: usize) -> Result<()> {
    let limit = ForeignMigrationPlan::MAX_CHAIN_LEVEL as usize;
    if current == 0 || !(2..=limit).contains(&added)
        || current.checked_add(added).is_none_or(|total| total > limit)
    {
        return Err(Invalid::Count.into());
    }
    Ok(())
}

struct SourceLock {
    root: DiskRoot,
    writer: std::fs::File,
    _directory: AdvisoryLock,
}
impl SourceLock {
    fn open(root: DiskRoot) -> Result<Self> {
        let directory = AdvisoryLock::directory(&root.directory(root.path())?)?;
        // No CREATE on the foreign source, not even for the normal writer lock.
        let writer = root.open_regular_entry(&layout::DISK_WRITER_LOCK)?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer).map_err(fs_error)?;
        pending::reservation::lock(&writer)?;
        let result = Self { root, writer, _directory: directory };
        result.recheck()?;
        Ok(result)
    }

    fn recheck(&self) -> Result<()> {
        pending::admission::verify_named_root(&self.root)?;
        let named = self.root.open_read_entry(&layout::DISK_WRITER_LOCK)?;
        let metadata = named.metadata()?;
        if metadata.len() != 0 || FileIdentity::from_fd(&self.writer)? != FileIdentity::from_metadata(&metadata) {
            return Err(Invalid::Binding.into());
        }
        Ok(())
    }
}

struct RootSet {
    paths: Vec<std::path::PathBuf>,
    inodes: BTreeSet<(u64, u64)>,
    devices: BTreeSet<u64>,
    production: bool,
}
impl RootSet {
    fn insert(&mut self, path: &Path, physical: FileIdentity, data: bool) -> Result<()> {
        if self.paths.iter().any(|prior| prior.starts_with(path) || path.starts_with(prior))
            || !self.inodes.insert((physical.device, physical.inode))
            || (data && self.production && !self.devices.insert(physical.device))
        {
            return Err(Invalid::Binding.into());
        }
        self.paths.push(path.into());
        Ok(())
    }
}

impl Engine {
    pub(super) fn observe_preserved_foreign_handoff(
        &self, request: ForeignIdentityTargetRequest, source: &ForeignImportPreview,
        principal: &str, directories: Vec<PendingMemberDirectory>,
    ) -> Result<()> {
        Self::require_actor(principal)?;
        self.require_write()?;
        self.require_handoff_source()?;
        require_count(self.config.disks.len(), directories.len())?;
        let operation = request.operation_id;
        let chain = request.chain_digest;
        let proposed = ValidatedConfig::try_from(request.proposed_configuration.clone())?;
        self.require_addition_chain(&proposed)?;
        let naming = self.capture_foreign_identity_target(request)?;
        // Retained additions have a root and directory lock apiece, plus the
        // source writer and the one lower capsule/ancestors under observation.
        // Existing pool descriptors have their own admission and real leases.
        let _descriptors = crate::engine::payload_proof_budget()
            .reserve(directories.len() * 2 + 8).ok_or(Invalid::Count)?;
        let mut selected = self.admit_added_directories(&proposed, directories)?;
        let mut added: Vec<_> = proposed.disks.iter().filter(|d| selected.contains_key(&d.disk_id)).collect();
        added.sort_by_key(|d| d.level);

        let _activity = self._locks.activity()?;
        let mut roots = RootSet {
            paths: Vec::new(), inodes: BTreeSet::new(), devices: BTreeSet::new(),
            production: self.config.mode == Mode::Production,
        };
        let system = DiskRoot::open(&self.root)?;
        let system_id = pending::admission::verify_named_root(&system)?;
        if system_id != self._locks.system_directory_identity()
            || FileIdentity::from_metadata(&system.open_read_entry(&layout::CONTROL_WRITER_LOCK)?.metadata()?)
                != self._locks.system_identity()?
        {
            return Err(Invalid::Binding.into());
        }
        roots.insert(system.path(), system_id, false)?;
        for disk in &self.config.disks {
            let current = self.disk_resources.get(disk)?;
            self._locks.verify_disk(disk, &current.root)?;
            crate::engine::provenance::validate_owned_seal(&self.disk_resources, disk, &self.metadata_integrity)?;
            self.config.verify_root_membership(&current.root, disk)?;
            roots.insert(&disk.root, pending::admission::verify_named_root(&current.root)?, true)?;
        }
        for root in selected.values() {
            roots.insert(root.path(), pending::admission::verify_named_root(root)?, true)?;
        }
        let highest = added.pop().ok_or(Invalid::Count)?;
        let high = SourceLock::open(selected.remove(&highest.disk_id).ok_or(Invalid::Binding)?)?;
        let mut lowers = Vec::with_capacity(added.len());
        for disk in added {
            let root = selected.remove(&disk.disk_id).ok_or(Invalid::Binding)?;
            lowers.push((disk.level, LockedCapsuleParent::capture(root)?));
        }
        // All original root locks remain held until the last late recheck;
        // decoded capsule plans/images need not be held for all roots at once.
        let highest_material = PreservedSourceInventory::capture(source, &high.root)?;
        let recipe = self.capture_foreign_admission_recipe(&naming, source)?;
        if &recipe.identities().original != highest_material.original() {
            return Err(Invalid::Binding.into());
        }
        recipe.verify_contract(&self.config, &proposed, operation)?;
        for (level, lower) in &lowers {
            let plan = highest_material.lower_plan(operation, &chain, *level)?;
            let owner = lower.observe(&operation, plan.digest())?;
            highest_material.capture_lower(&owner, &plan)?;
        }
        highest_material.recheck()?;
        high.recheck()?;
        for (_, lower) in &lowers { lower.recheck()?; }
        self.recheck_foreign_identity_target(&naming)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/foreign_import/preserved_root_budget_tests.rs"]
mod tests;
