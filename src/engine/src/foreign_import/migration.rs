//! Offline migration capsules. Publication here seals a reviewable, independently
//! verifiable copy; it does not activate an Engine membership or rewrite a seal.
use super::*;
use data_serialization::{Deserialize, Serialize};
use name_policy::PortablePath;
use platform_io::fs_safety::{EntryName, RelativePath, linux::AdvisoryLock};
use std::{collections::BTreeSet, io::Read, path::Path};

mod inventory;
mod planning;
mod projection;
mod transaction;

const FORMAT: &str = "zxs-foreign-migration-capsule";
const MAX_INTENT: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct PhysicalBinding {
    device: u64,
    inode: u64,
}
impl PhysicalBinding {
    pub(super) fn of(root: &DiskRoot) -> Result<Self> {
        let identity = root.directory_identity(root.path())?;
        Ok(Self {
            device: identity.device,
            inode: identity.inode,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct FileCommitment {
    bytes: u64,
    digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Inventory {
    directories: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, FileCommitment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Origin {
    pool_id: String,
    disk_id: String,
    slot_id: String,
    binding_generation: u64,
}
impl From<&ForeignPrimaryOrigin> for Origin {
    fn from(origin: &ForeignPrimaryOrigin) -> Self {
        Self {
            pool_id: origin.pool_id.clone(),
            disk_id: origin.disk_id.clone(),
            slot_id: origin.slot_id.clone(),
            binding_generation: origin.binding_generation,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct OriginGroup {
    origin: Origin,
    directory: String,
    histories: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct AcceptedPrimary {
    registration: DiskRegistrationIdentity,
    sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Placement {
    Primary,
    /// Isolated material, not an admitted role or membership decision.
    ChainMaterial {
        chain_digest: String,
        target_level: u32,
    },
    Rebuild {
        primary: AcceptedPrimary,
        directory: String,
    },
    Secondary {
        primary: AcceptedPrimary,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct InspectionBudget {
    metadata_bytes: usize,
    journal_bytes: usize,
    versions: usize,
    payload_bytes: u64,
}
impl InspectionBudget {
    pub(super) fn from_limits(limits: ForeignImportLimits) -> Self {
        Self {
            metadata_bytes: limits.metadata_bytes,
            journal_bytes: limits.journal_bytes,
            versions: limits.versions,
            payload_bytes: limits.payload_bytes,
        }
    }
    pub(super) fn limits(&self) -> Result<ForeignImportLimits> {
        ForeignImportLimits::new(
            self.metadata_bytes,
            self.journal_bytes,
            self.versions,
            self.payload_bytes,
        )
    }
    fn total_bytes(&self) -> u64 {
        // limits() proves each operand <= 1 TiB, so this sum cannot overflow.
        self.payload_bytes + self.metadata_bytes as u64 + self.journal_bytes as u64
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PlanRecord {
    format: String,
    operation_id: OperationId,
    source_digest: String,
    source_registration: DiskRegistrationIdentity,
    source_level: u32,
    source_path: PathBuf,
    source_local: PhysicalBinding,
    placement: Placement,
    groups: Vec<OriginGroup>,
    budget: InspectionBudget,
    inventory: Inventory,
}

/// Frozen role, provenance, namespace preconditions and exact raw-file inventory.
/// Construction reads and hashes a source; it never changes source or Engine state.
pub struct ForeignMigrationPlan {
    record: PlanRecord,
    digest: ForeignEvidenceDigest,
}
impl ForeignMigrationPlan {
    pub fn digest(&self) -> &ForeignEvidenceDigest {
        &self.digest
    }
    pub fn operation_id(&self) -> &OperationId {
        &self.record.operation_id
    }
    pub fn origin_count(&self) -> usize {
        self.record.groups.len()
    }
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    plan: PlanRecord,
    destination: String,
    parent_path: PathBuf,
    parent: PhysicalBinding,
}

/// Exclusive owner of one staged or already published migration capsule.
/// Drop leaves all durable material for explicit resume; it never rolls back.
pub struct ForeignMigrationTransaction {
    parent: DiskRoot,
    capsule: DiskRoot,
    intent: Intent,
    digest: ForeignEvidenceDigest,
    published: bool,
    _lock: AdvisoryLock,
}

/// Read-only local progress. Reserved includes incomplete staging windows.
/// None of these states grants active storage membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignMigrationStatus {
    Unstarted,
    Reserved,
    Published,
}

/// A synced offline capsule, not an active disk-registration receipt.
pub struct ForeignMigrationReceipt {
    path: PathBuf,
    digest: ForeignEvidenceDigest,
    origin_count: usize,
}
impl ForeignMigrationReceipt {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn digest(&self) -> &ForeignEvidenceDigest {
        &self.digest
    }
    pub fn origin_count(&self) -> usize {
        self.origin_count
    }
}

fn digest(record: &PlanRecord) -> Result<ForeignEvidenceDigest> {
    object_hash("foreign-migration-plan", record)?
        .parse()
        .map_err(|error| Error::from(std::io::Error::other(error)))
}

fn changed(message: &str) -> Error {
    Error::new(ErrorCode::ObjectCollision, message)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/foreign_import/migration_tests.rs"]
mod tests;
