//! A derived, current-binding disk inventory, not a disk-local event history or
//! an independently recoverable publication. No unregistered root is opened.
use super::Engine;
use crate::{
    database_install::{DISK_DATABASE_NAME, InstallAdmission, PreparedInstall, Scope},
    disk::{validate_seal_observed, validate_stored_seal},
    *,
};
use data_serialization::{Deserialize, Serialize};
use std::sync::Arc;

mod database;
mod directories;
pub use directories::DiskCatalogDirectory;
mod history;
mod maintenance;
pub(in crate::engine) mod root_install;
pub(in crate::engine) use history::preparation::PreparedDiskScopes;
pub use history::{
    DiskDatabaseMaterial, DiskDatabaseReadScope, DiskDatabaseRelation, DiskHistoryChange,
    DiskHistoryFailure, DiskHistoryHeader, DiskHistoryMaintenanceState, DiskHistoryObservation,
    DiskHistoryReceipt, DiskHistoryRecord, DiskHistorySnapshot, DiskHistoryStatus,
    inspect_disk_database_material,
};
pub(super) use maintenance::Maintenance;
pub use maintenance::{DiskCatalogFailure, DiskCatalogMaintenanceState, DiskCatalogStatus};

const MAX_ROWS: usize = 65_536;
const MAX_RECORD_BYTES: usize = 24 * 1024 * 1024;
const MAX_DATABASE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
pub enum DiskCatalogRole {
    Primary,
    Backup,
}

/// A local placement obligation and its current owner name. Missing owner_path
/// remains unknown, and content_state is logical disposition, not live health.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskCatalogVersion {
    pub version: FileVersion,
    pub role: DiskCatalogRole,
    pub owner_path: Option<String>,
    pub content_state: Option<ContentState>,
}

/// Scope is the exact physical disk registration, never just its reusable slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskCatalogSnapshot {
    pub identity: DiskRegistrationIdentity,
    #[serialization(with = "storage_contract::u64_string")]
    pub source_sequence: u64,
    pub versions: Vec<DiskCatalogVersion>,
    pub backup_directories: Vec<DiskCatalogDirectory>,
}

impl DiskCatalogSnapshot {
    fn empty(identity: DiskRegistrationIdentity) -> Self {
        Self {
            identity,
            source_sequence: 0,
            versions: Vec::new(),
            backup_directories: Vec::new(),
        }
    }

    fn same_rows(&self, other: &Self) -> bool {
        self.versions == other.versions && self.backup_directories == other.backup_directories
    }

    fn is_empty(&self) -> bool {
        self.versions.is_empty() && self.backup_directories.is_empty()
    }
}

/// Distinguish when the bytes were materialized from the admitted State used
/// for this read. An unrelated global publication need not rewrite this disk.
#[derive(Debug)]
pub struct DiskCatalogObservation {
    pub verified_sequence: u64,
    pub snapshot: DiskCatalogSnapshot,
}

#[derive(Debug)]
pub enum DiskCatalogRefreshOutcome {
    Updated(DiskCatalogReceipt),
    Failed(Error),
}

#[derive(Debug)]
pub struct DiskCatalogRefreshResult {
    pub slot_id: String,
    pub outcome: DiskCatalogRefreshOutcome,
}

/// Explicit partial outcomes: a successful earlier disk is not rolled back
/// when a later disk is unavailable or contains an unknown object.
#[derive(Debug)]
pub struct DiskCatalogRefreshReport {
    pub source_sequence: u64,
    pub results: Vec<DiskCatalogRefreshResult>,
}

#[derive(Debug)]
pub struct DiskCatalogReceipt {
    pub identity: DiskRegistrationIdentity,
    pub source_sequence: u64,
    pub versions: usize,
    pub backup_directories: usize,
    pub records_hash: String,
    pub database_bytes: usize,
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "disk catalog exceeds its bounded record or byte budget",
    )
}

fn snapshot(state: &State, identity: DiskRegistrationIdentity) -> Result<DiskCatalogSnapshot> {
    let mut versions = Vec::new();
    let mut bytes = 0usize;
    for (id, placement) in state.placements.iter() {
        if !placement.route_slot_ids.contains(&identity.slot_id)
            || placement.binding_generations.get(&identity.slot_id)
                != Some(&identity.binding_generation)
        {
            continue;
        }
        let version = state
            .versions
            .get(id)
            .expect("invariant: admitted placement references a retained version");
        let (role, owner_path) = if placement.primary_slot_id == identity.slot_id {
            (
                DiskCatalogRole::Primary,
                state.file_locations.get(&version.file_id).cloned(),
            )
        } else {
            (
                DiskCatalogRole::Backup,
                state
                    .file_backup_paths
                    .get(&version.file_id)
                    .and_then(|paths| paths.get(&identity.slot_id))
                    .cloned(),
            )
        };
        let row = DiskCatalogVersion {
            version: version.clone(),
            role,
            owner_path,
            content_state: state.content_states.get(id).copied(),
        };
        bytes += crate::disk::encode_metadata(&row, MAX_RECORD_BYTES)?.len();
        if versions.len() == MAX_ROWS || bytes > MAX_RECORD_BYTES {
            return Err(budget());
        }
        versions.push(row);
    }
    let backup_directories = directories::snapshot(state, &identity, versions.len(), &mut bytes)?;
    Ok(DiskCatalogSnapshot {
        identity,
        source_sequence: state.sequence,
        versions,
        backup_directories,
    })
}

impl Engine {
    fn catalog_disk(
        &self,
        slot: &str,
    ) -> Result<(
        DiskConfig,
        DiskRegistrationIdentity,
        Arc<crate::disk::DiskHandle>,
    )> {
        let registration = self
            .registered_disks()
            .find(|disk| disk.slot_id == slot)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskUnavailable,
                    "disk catalog requires a currently registered disk",
                )
            })?;
        if registration.registration_mode != RegistrationMode::Enabled {
            return Err(Error::new(
                ErrorCode::DiskUnavailable,
                "disk catalog cannot use a disabled registration",
            ));
        }
        let disk = self.disk(slot)?.clone();
        let identity = registration.identity();
        if disk.disk_id != identity.disk_id
            || disk.binding_generation != identity.binding_generation
            || self.config.pool_id != identity.pool_id
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "catalog registration differs from admitted I/O binding",
            ));
        }
        let handle = Arc::clone(self.disk_resources.get(&disk)?);
        // A preserved Backup root-name switch is not the old Engine's State.
        // Its selection only fences these disposable reads/maintenance paths;
        // it cannot select an authority or grant a successor writer.
        root_install::require_unselected(&handle.root)?;
        Ok((disk, identity, handle))
    }

    /// Explicitly materialize this disk's current version and recursive-directory obligations.
    /// Requires exclusive Engine ownership and an already admitted pool/disk.
    /// It neither adds/initializes a disk nor certifies local body availability.
    /// The caller must authorize a global maintenance write. An interrupted
    /// attempt is safely repeated from the same admitted State and held root.
    pub fn rebuild_disk_catalog(&mut self, slot: &str) -> Result<DiskCatalogReceipt> {
        self.require_write()?;
        let (disk, identity, handle) = self.catalog_disk(slot)?;
        let lease = handle.root.admit_io()?;
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        let captured = snapshot(&self.state, identity.clone())?;
        let records_hash = storage_contract::object_hash("disk-current-catalog", &captured)?;
        let bytes = database::encode(&captured, &records_hash)?;
        let database_bytes = bytes.len();
        let root = &handle.root;
        let inode = root.directory_identity(root.path())?;
        let prepared = PreparedInstall::capture(
            root,
            root.path(),
            (root.path(), inode),
            (
                Scope::disk(identity.clone()),
                InstallAdmission::NewOperation,
            ),
            bytes,
            MAX_DATABASE_BYTES,
            |bytes| database::read(bytes, &identity).map(|_| ()),
        )?;
        let (reserved, entries) = prepared.reservation();
        InstallAdmission::NewOperation.check(root, root.path(), reserved, entries)?;
        if reserved.saturating_add(self.config.metadata_reserve_bytes) > root.available()? {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "disk catalog must preserve metadata recovery headroom",
            ));
        }
        prepared.apply(root, &mut |_| lease.check_running())?;
        let stored = root.read_bytes(&root.path().join(DISK_DATABASE_NAME), MAX_DATABASE_BYTES)?;
        if database::read(&stored, &identity)? != captured {
            return Err(invalid(
                "installed disk catalog differs from its admitted snapshot",
            ));
        }
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        lease.check_running()?;
        let receipt = DiskCatalogReceipt {
            identity,
            source_sequence: captured.source_sequence,
            versions: captured.versions.len(),
            backup_directories: captured.backup_directories.len(),
            records_hash,
            database_bytes,
        };
        self.record_catalog_install(&receipt);
        Ok(receipt)
    }

    /// Read complete current local rows. The stored materialization sequence
    /// may precede this State when only other disks/global metadata changed.
    pub fn disk_catalog_snapshot(&self, slot: &str) -> Result<DiskCatalogSnapshot> {
        Ok(self.disk_catalog_observation(slot)?.snapshot)
    }

    /// Validate the complete local row set against the currently admitted State.
    /// No sequence-only shortcut can turn a changed local path, placement or
    /// content disposition into an unchanged result.
    pub fn disk_catalog_observation(&self, slot: &str) -> Result<DiskCatalogObservation> {
        let (disk, identity, handle) = self.catalog_disk(slot)?;
        let lease = handle.root.admit_io()?;
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        let expected = snapshot(&self.state, identity.clone())?;
        let bytes = handle.root.read_bytes(
            &handle.root.path().join(DISK_DATABASE_NAME),
            MAX_DATABASE_BYTES,
        )?;
        let stored = database::read(&bytes, &identity)?;
        if stored.source_sequence > expected.source_sequence
            || stored.identity != expected.identity
            || !stored.same_rows(&expected)
        {
            return Err(invalid(
                "disk catalog local rows differ from admitted State or claim a future sequence; explicitly rebuild it",
            ));
        }
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        lease.check_running()?;
        Ok(DiskCatalogObservation {
            verified_sequence: expected.source_sequence,
            snapshot: stored,
        })
    }

    /// Refresh an explicit bounded set of registered disk catalogs under this
    /// Engine's writer ownership. No target discovery or first-run initialization
    /// occurs. Invalid/duplicate target lists are rejected before any writes;
    /// per-disk I/O failures remain visible and do not hide completed targets.
    pub fn refresh_disk_catalogs(&mut self, slots: &[String]) -> Result<DiskCatalogRefreshReport> {
        if slots.is_empty() || slots.len() > 256 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "disk catalog refresh requires 1..=256 explicit slots",
            ));
        }
        let mut unique = std::collections::BTreeSet::new();
        for slot in slots {
            if !unique.insert(slot) {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "disk catalog refresh contains duplicate slots",
                ));
            }
            // Check every logical target before making the first side effect.
            // Runtime I/O still revalidates each target immediately before use.
            self.catalog_disk(slot)?;
        }
        self.require_write()?;
        let source_sequence = self.state.sequence;
        let results = slots
            .iter()
            .map(|slot| DiskCatalogRefreshResult {
                slot_id: slot.clone(),
                outcome: match self.rebuild_disk_catalog(slot) {
                    Ok(receipt) => DiskCatalogRefreshOutcome::Updated(receipt),
                    Err(error) => DiskCatalogRefreshOutcome::Failed(error),
                },
            })
            .collect();
        Ok(DiskCatalogRefreshReport {
            source_sequence,
            results,
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/disk_catalog_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/disk_catalog_directory_tests.rs"]
mod directory_tests;
