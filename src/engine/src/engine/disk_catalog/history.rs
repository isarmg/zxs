//! Disk-local changes reconstructed from published before/after States, not
//! attributed to a disk using today's placement. This projection has no vote.
use super::*;
use crate::database_install::DISK_HISTORY_DATABASE_NAME;
use std::collections::BTreeMap;

mod database;
mod stored;
pub(in crate::engine) use stored::{StoredHistory, Through};
mod maintenance;
mod offline;
pub(super) use offline::fold_stored as fold_prepared;
#[cfg(test)]
pub(super) use offline::fold as fold_export;
pub(in crate::engine::disk_catalog) mod preparation;
mod replay;
pub(super) use maintenance::Maintenance;
pub use maintenance::{DiskHistoryFailure, DiskHistoryMaintenanceState, DiskHistoryStatus};
pub use offline::{
    DiskDatabaseMaterial, DiskDatabaseReadScope, DiskDatabaseRebuildSource,
    DiskDatabaseReconstruction, DiskDatabaseRelation, MAX_DISK_DATABASE_ARCHIVE_BYTES,
    inspect_disk_database_material, reconstruct_disk_database_material,
};
pub(in crate::engine) use replay::{PublishedSnapshot, visit_published_prefix};

const MAX_PUBLICATIONS: usize = 2048;
const MAX_CHANGES: usize = 65_536;

/// Decode only this exact disk's retained representation. The installer owner
/// separately authenticates the new bytes from the native publication.
pub(super) fn read_for_root_install(
    bytes: &[u8],
    identity: &DiskRegistrationIdentity,
) -> Result<StoredHistory> {
    database::read(bytes, identity)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DiskHistoryChange {
    BackupDirectoryEntered {
        after: DiskCatalogDirectory,
    },
    BackupDirectoryChanged {
        before: DiskCatalogDirectory,
        after: DiskCatalogDirectory,
    },
    BackupDirectoryDeparted {
        before: DiskCatalogDirectory,
    },
    Entered {
        after: DiskCatalogVersion,
    },
    Changed {
        before: Box<DiskCatalogVersion>,
        after: DiskCatalogVersion,
    },
    Departed {
        before: DiskCatalogVersion,
    },
}
impl DiskHistoryChange {
    /// File changes have a version identity; directory-only changes never
    /// manufacture a zero-byte version or borrow a descendant's identity.
    pub fn version_id(&self) -> Option<&str> {
        match self {
            Self::Entered { after } | Self::Changed { after, .. } => {
                Some(&after.version.version_id)
            }
            Self::Departed { before } => Some(&before.version.version_id),
            Self::BackupDirectoryEntered { .. }
            | Self::BackupDirectoryChanged { .. }
            | Self::BackupDirectoryDeparted { .. } => None,
        }
    }

    fn key(&self) -> (u8, &str, &str, &str, &str) {
        match self {
            Self::Entered { after } | Self::Changed { after, .. } => {
                (1, &after.version.version_id, "", "", "")
            }
            Self::Departed { before } => (1, &before.version.version_id, "", "", ""),
            Self::BackupDirectoryEntered { after } | Self::BackupDirectoryChanged { after, .. } => {
                let (repository, history, directory, path) = after.key();
                (0, repository, history, directory, path)
            }
            Self::BackupDirectoryDeparted { before } => {
                let (repository, history, directory, path) = before.key();
                (0, repository, history, directory, path)
            }
        }
    }
}

/// Only local row changes are persisted. The operation ID and hashes identify
/// their source publication; unrelated global operation payloads are not copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskHistoryRecord {
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub operation_id: OperationId,
    pub candidate_hash: String,
    pub snapshot_hash: String,
    pub change: DiskHistoryChange,
}

/// Coverage starts at the admitted empty genesis, not at the first local row.
/// Nonlocal publications are verified but do not produce local change rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskHistoryHeader {
    pub identity: DiskRegistrationIdentity,
    #[serialization(with = "storage_contract::u64_string")]
    pub through_sequence: u64,
    pub through_snapshot: Option<String>,
    pub publications: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskHistorySnapshot {
    pub header: DiskHistoryHeader,
    pub changes: Vec<DiskHistoryRecord>,
}

/// verified_sequence is the observed Engine head. Stored coverage ends at
/// snapshot.header.through_sequence, which may be older and is never relabeled.
#[derive(Debug)]
pub struct DiskHistoryObservation {
    pub verified_sequence: u64,
    pub snapshot: DiskHistorySnapshot,
}

#[derive(Debug)]
pub struct DiskHistoryReceipt {
    pub header: DiskHistoryHeader,
    pub changes: usize,
    pub records_hash: String,
    pub database_bytes: usize,
    binding: Binding,
}

/// Boundaries supplied by the completed replay or the completed native write;
/// an isolated SQLite is never allowed to construct a resolved current source.
#[derive(Clone, Debug)]
struct Binding {
    through: Through,
    candidate: Option<String>,
}
impl Binding {
    fn genesis() -> Self { Self { through: Through::Genesis {}, candidate: None } }
    fn published(config: &ValidatedConfig, candidate: &crate::journal::Candidate, hash: &str) -> Result<Self> {
        Ok(Self { through: Through::from_candidate(config, candidate)?, candidate: Some(hash.into()) })
    }
}
struct CapturedHistory {
    view: DiskHistorySnapshot,
    binding: Binding,
}
struct Changes {
    rows: Vec<DiskHistoryRecord>,
    bytes: usize,
}
impl Changes {
    fn retained(rows: Vec<DiskHistoryRecord>) -> Result<Self> {
        let mut changes = Self { rows, bytes: 0 };
        for record in &changes.rows {
            let bytes = crate::disk::encode_metadata(record, MAX_RECORD_BYTES)?.len();
            if bytes > MAX_RECORD_BYTES - changes.bytes {
                return Err(budget());
            }
            changes.bytes += bytes;
        }
        Ok(changes)
    }

    fn append(
        &mut self,
        candidate: &crate::journal::Candidate,
        hash: &str,
        before: &DiskCatalogSnapshot,
        after: &DiskCatalogSnapshot,
    ) -> Result<()> {
        visit_changes(before, after, |change| self.push(candidate, hash, change))
    }

    fn push(
        &mut self,
        candidate: &crate::journal::Candidate,
        hash: &str,
        change: DiskHistoryChange,
    ) -> Result<()> {
        let record = DiskHistoryRecord {
            sequence: candidate.sequence,
            operation_id: candidate.operation_id,
            candidate_hash: hash.into(),
            snapshot_hash: candidate.snapshot_hash.clone(),
            change,
        };
        let bytes = crate::disk::encode_metadata(&record, MAX_RECORD_BYTES)?.len();
        if self.rows.len() == MAX_CHANGES || bytes > MAX_RECORD_BYTES - self.bytes {
            return Err(budget());
        }
        self.bytes += bytes;
        self.rows.push(record);
        Ok(())
    }
}

/// One deterministic source of local changes for already-decided replay and
/// prospective physical preparation. The delta itself has no enclosing hashes.
fn visit_changes(
    before: &DiskCatalogSnapshot, after: &DiskCatalogSnapshot,
    mut visit: impl FnMut(DiskHistoryChange) -> Result<()>,
) -> Result<()> {
        let mut directories = BTreeMap::new();
        for row in &before.backup_directories {
            directories.insert(row.key(), (Some(row), None));
        }
        for row in &after.backup_directories {
            directories.entry(row.key()).or_insert((None, None)).1 = Some(row);
        }
        for (before, after) in directories.into_values() {
            let change = match (before, after) {
                (Some(before), Some(after)) if before == after => continue,
                (Some(before), Some(after)) => DiskHistoryChange::BackupDirectoryChanged {
                    before: before.clone(),
                    after: after.clone(),
                },
                (Some(before), None) => DiskHistoryChange::BackupDirectoryDeparted {
                    before: before.clone(),
                },
                (None, Some(after)) => DiskHistoryChange::BackupDirectoryEntered {
                    after: after.clone(),
                },
                (None, None) => unreachable!("invariant: every union entry has a local row"),
            };
            visit(change)?;
        }
        let mut rows = BTreeMap::new();
        for row in &before.versions {
            rows.insert(row.version.version_id.as_str(), (Some(row), None));
        }
        for row in &after.versions {
            rows.entry(row.version.version_id.as_str())
                .or_insert((None, None))
                .1 = Some(row);
        }
        for (before, after) in rows.into_values() {
            let change = match (before, after) {
                (Some(before), Some(after)) if before == after => continue,
                (Some(before), Some(after)) => DiskHistoryChange::Changed {
                    before: Box::new(before.clone()),
                    after: after.clone(),
                },
                (Some(before), None) => DiskHistoryChange::Departed {
                    before: before.clone(),
                },
                (None, Some(after)) => DiskHistoryChange::Entered {
                    after: after.clone(),
                },
                (None, None) => unreachable!("invariant: every union entry has a local row"),
            };
            visit(change)?;
        }
        Ok(())
    }


impl Engine {
    /// Reconstruct a bounded disk-local history from every published State in
    /// the admitted genesis chain and install it at the registered disk root.
    /// Completed Backup generations replay their independently verified archive
    /// and exact first publication before the ordinary suffix. Other generation
    /// kinds remain unsupported rather than relabelling their historical disks.
    /// Callers must authorize maintenance; this never admits or initializes a pool.
    pub fn rebuild_disk_history(&mut self, slot: &str) -> Result<DiskHistoryReceipt> {
        self.require_write()?;
        // A failed attempt cannot leave an earlier in-memory success label in
        // place. The caller retains its exact error; automatic callers latch it.
        self.forget_disk_history(slot);
        let (_, identity, _) = self.catalog_disk(slot)?;
        let captured = replay::capture(self, identity, self.state.sequence)?;
        let receipt = self.install_disk_history(slot, captured)?;
        self.record_history_install(&receipt);
        Ok(receipt)
    }

    fn install_disk_history(
        &self,
        slot: &str,
        captured: CapturedHistory,
    ) -> Result<DiskHistoryReceipt> {
        let (disk, identity, handle) = self.catalog_disk(slot)?;
        let CapturedHistory { view, binding } = captured;
        if view.header.identity != identity {
            return Err(invalid("disk history target registration changed"));
        }
        let lease = handle.root.admit_io()?;
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        let captured = StoredHistory::from_replayed(&view, binding.through)?;
        let records_hash = captured.logical_root()?.hash().to_string();
        let bytes = database::encode(&captured)?;
        let database_bytes = bytes.len();
        let root = &handle.root;
        let inode = root.directory_identity(root.path())?;
        let prepared = PreparedInstall::capture(
            root,
            root.path(),
            (root.path(), inode),
            (
                Scope::disk_history(identity.clone()),
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
                "disk history must preserve metadata recovery headroom",
            ));
        }
        prepared.apply(root, &mut |_| lease.check_running())?;
        let bytes = root.read_bytes(
            &root.path().join(DISK_HISTORY_DATABASE_NAME),
            MAX_DATABASE_BYTES,
        )?;
        if database::read(&bytes, &identity)? != captured {
            return Err(invalid(
                "installed disk history differs from authenticated publications",
            ));
        }
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        lease.check_running()?;
        Ok(DiskHistoryReceipt {
            header: view.header,
            changes: view.changes.len(),
            binding,
            records_hash,
            database_bytes,
        })
    }

    /// Read and rederive the stored historical coverage against authoritative
    /// journals/snapshots. An older complete prefix is explicitly returned as
    /// older coverage, not as a current or independently recoverable catalog.
    /// This read never repairs, truncates or adopts the stored database.
    pub fn disk_history_observation(&self, slot: &str) -> Result<DiskHistoryObservation> {
        let material = self.disk_databases_from_original(slot, DiskDatabaseReadScope::History)?;
        Ok(DiskHistoryObservation {
            verified_sequence: self.state.sequence,
            snapshot: material.into_history().ok_or_else(|| invalid("selected original history is absent"))?,
        })
    }

    /// The on-disk coverage is a requested boundary, never an election. Read
    /// through the original descriptor while the entire native prefix is verified.
    pub(in crate::engine::disk_catalog) fn disk_databases_from_original(
        &self, slot: &str, scope: DiskDatabaseReadScope,
    ) -> Result<DiskDatabaseMaterial> {
        let (disk, identity, handle) = self.catalog_disk(slot)?;
        let lease = handle.root.admit_io()?;
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        let material = offline::inspect_with_original(
            &handle.root, &identity, scope, |stored| {
                let expected = replay::capture(self, identity.clone(), stored.header.through.sequence())?;
                if stored != StoredHistory::from_replayed(&expected.view, expected.binding.through)? {
                    return Err(invalid("disk history differs from its authenticated publication prefix"));
                }
                Ok(expected.view)
            },
        )?;
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        lease.check_running()?;
        Ok(material)
    }
}
