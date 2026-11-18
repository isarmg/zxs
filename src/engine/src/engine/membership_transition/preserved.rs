//! The complete Foreign action has a separate, read-only proof path. Ordinary
//! Add's root reservations and fence cannot express preserved source identity.
use super::*;
use crate::foreign_import::{LockedCapsuleParent, PreservedSourceInventory};
use crate::persist::layout;
use platform_io::fs_safety::linux::{AdvisoryLock, FileIdentity};
use std::collections::BTreeSet;
use std::path::Path;

mod record;

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
    #[error("an immutable Foreign enrollment record is not the exact freshly observed original")]
    Original,
    #[error("the canonical Foreign enrollment record exceeds 64 KiB")]
    RecordBudget,
    #[error("the Foreign action has no principal")]
    Principal(#[source] storage_contract::EmptyPrincipal),
}
impl From<Invalid> for Error {
    fn from(failure: Invalid) -> Self {
        let code = match failure {
            Invalid::Count | Invalid::RecordBudget => ErrorCode::ResourceLimit,
            Invalid::Binding => ErrorCode::DiskIdentityMismatch,
            Invalid::Original => ErrorCode::IdempotencyKeyReused,
            Invalid::Principal(_) => ErrorCode::Unauthenticated,
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
    ) -> Result<record::Observed> {
        if principal.len() > record::MAX_RECORD_BYTES {
            return Err(Invalid::RecordBudget.into());
        }
        let actor = principal.parse().map_err(Invalid::Principal)?;
        self.require_write()?;
        self.require_handoff_source()?;
        require_count(self.config.disks.len(), directories.len())?;
        let operation = request.operation_id;
        let chain = request.chain_digest;
        let reviewed = record::RequestDigests::original(&request);
        let naming = self.capture_preserved_foreign_identity_target(&request)?;
        let proposed = &request.proposed_configuration;
        self.require_addition_chain(proposed)?;
        // Retained additions have a root and directory lock apiece, plus the
        // source writer and the one lower capsule/ancestors under observation.
        // Existing pool descriptors have their own admission and real leases.
        let _descriptors = crate::engine::payload_proof_budget()
            .reserve(directories.len() * 2 + 8).ok_or(Invalid::Count)?;
        let mut selected = self.admit_added_directories(proposed, directories)?;
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
            crate::engine::fresh_layout::verify_root(&self.config, disk, &current.root)?;
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
            let physical = pending::admission::verify_named_root(&root)?;
            lowers.push((disk, physical, LockedCapsuleParent::capture(root)?));
        }
        // All original root locks remain held until the last late recheck;
        // decoded capsule plans/images need not be held for all roots at once.
        let highest_material = PreservedSourceInventory::capture(source, &high.root)?;
        let recipe = self.capture_foreign_admission_recipe(&naming, source)?;
        if &recipe.identities().original != highest_material.original() {
            return Err(Invalid::Binding.into());
        }
        // Exercise the same complete single-origin lifecycle/route refusals
        // as a real joint recipe. This computes no vote or on-disk next State.
        recipe.joint_state(&self.state, &self.config, proposed, self.next_slot)?;
        let mut ordered = Vec::with_capacity(lowers.len() + 1);
        for (disk, physical, lower) in &lowers {
            let plan = highest_material.lower_plan(operation, &chain, disk.level)?;
            let owner = lower.observe(&operation, plan.digest())?;
            ordered.push(record::Root::observed(
                proposed, disk, *physical, highest_material.capture_lower(&owner, &plan)?,
            )?);
        }
        ordered.push(record::Root::observed(
            proposed,
            highest,
            pending::admission::verify_named_root(&high.root)?,
            highest_material.material().clone(),
        )?);
        highest_material.recheck()?;
        high.recheck()?;
        for (_, _, lower) in &lowers {
            lower.recheck()?;
        }
        self.recheck_foreign_identity_target(&naming)?;
        let snapshot = source.preserved_current_snapshot(self)?;
        let observed = record::Observed::freeze(actor, reviewed, snapshot, &recipe, ordered)?;
        self.recheck_preserved_roots(&system)?;
        observed.require_original_if_present(&system)?;
        Ok(observed)
    }

    pub(crate) fn require_uninstalled_foreign_proposal(&self, proposed: &Config) -> Result<()> {
        if self.config.storage_roots.is_empty()
            || proposed.storage_roots != self.config.storage_roots
        {
            return Err(Invalid::Binding.into());
        }
        // Check ID, topology, names, overlaps and all normal pool settings
        // without constructing a Config admitted for target disk I/O.
        crate::engine::config::validate_addition_input(proposed)?;
        pending::admission::validate_addition_members(&self.config, proposed, &self.root)?;
        Ok(())
    }

    fn recheck_preserved_roots(&self, system: &DiskRoot) -> Result<()> {
        if pending::admission::verify_named_root(system)? != self._locks.system_directory_identity()
            || FileIdentity::from_metadata(
                &system.open_read_entry(&layout::CONTROL_WRITER_LOCK)?.metadata()?
            ) != self._locks.system_identity()?
        {
            return Err(Invalid::Binding.into());
        }
        for disk in &self.config.disks {
            let handle = self.disk_resources.get(disk)?;
            self._locks.verify_disk(disk, &handle.root)?;
            pending::admission::verify_named_root(&handle.root)?;
            crate::engine::provenance::validate_owned_seal(
                &self.disk_resources, disk, &self.metadata_integrity,
            )?;
            self.config.verify_root_membership(&handle.root, disk)?;
            crate::engine::fresh_layout::verify_root(&self.config, disk, &handle.root)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/foreign_import/preserved_root_budget_tests.rs"]
mod tests;
