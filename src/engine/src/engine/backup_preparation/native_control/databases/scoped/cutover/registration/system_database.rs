//! Preserve the closed old SYSTEM main/WAL/SHM/journal set before installing
//! the exact published index at its ordinary name. This is not active Current.
use super::*;
use crate::engine::index::backup_install::{QuiescentIndex, SELECTION};
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

mod retention;
use retention::Selected;

const COMPLETE: &str = "backup-system-index-installed.json";
const MAX_DESCRIPTOR: usize = 128 * 1024;
const MAX_DATABASE: usize = 64 * 1024 * 1024;
const MARGIN: u64 = 1024 * 1024;

/// The ordinary SYSTEM index and all ordinary disk-root pairs were compared
/// with the original actual Publication. Old SQLite files remain retained;
/// this report grants neither active Current, a writer nor source release.
#[derive(Clone, Debug)]
pub struct BackupSystemDatabaseReport {
    pub roots: BackupRootDatabasesReport,
    pub installation_hash: String,
    pub system_database_hash: String,
    pub system_database_bytes: u64,
    pub retained_old_files: usize,
}

#[derive(Serialize)]
struct Binding<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    publication_hash: &'a str,
    root_installation_hash: &'a str,
    system: &'a PhysicalBinding,
    system_writer: &'a PhysicalBinding,
    database_hash: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    database_bytes: u64,
}
struct Plan {
    roots: BackupRootDatabasesReport,
    binding_hash: String,
    database: Vec<u8>,
    reserve: u64,
}
impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        // Root observation includes all original real Publication slots, every
        // selected target's independent full material and both ordinary pairs.
        let roots = root_databases::observe(context, prepared, decision, expected, stop)?;
        let common = &roots.aliases.current.common;
        context.sources.recheck(context.system)?;
        let database = context.system.read_bytes(
            &context
                .system
                .path()
                .join("backup-common-databases/index.sqlite"),
            MAX_DATABASE,
        )?;
        if database.len() as u64 != common.system_database_bytes
            || blake3::hash(&database).to_hex().as_str() != common.system_database_hash
        {
            return Err(invalid(
                "SYSTEM installation differs from the exact published database",
            ));
        }
        let bytes = canonical_json(&Binding {
            format: "zxs-backup-ordinary-system-index",
            source_fence_hash: prepared.source_fence_hash(),
            publication_hash: expected.as_str(),
            root_installation_hash: &roots.installation_hash,
            system: context.sources.system_binding(),
            system_writer: context.sources.system_writer_binding(),
            database_hash: &common.system_database_hash,
            database_bytes: common.system_database_bytes,
        })?;
        if bytes.len() > MAX_DESCRIPTOR {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "SYSTEM installation descriptor exceeds its budget",
            ));
        }
        let (_, candidate) = first_candidate::read_verified(prepared, decision, stop)?;
        context
            .choices
            .observe_first_publication(prepared, &candidate, stop)?;
        context.sources.recheck(context.system)?;
        Ok(Self {
            binding_hash: blake3::hash(&bytes).to_hex().to_string(),
            database,
            reserve: candidate.original_config().metadata_reserve_bytes,
            roots,
        })
    }
    fn require_capacity(&self, system: &DiskRoot) -> Result<()> {
        let needed = (self.database.len() as u64)
            .checked_add(self.reserve)
            .and_then(|n| n.checked_add(MARGIN + 2 * MAX_DESCRIPTOR as u64))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "SYSTEM installation space overflow",
                )
            })?;
        if system.available()? < needed {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "SYSTEM cannot retain old SQLite and independently install its published replacement",
            ));
        }
        Ok(())
    }
    fn report(self, selected: &Selected) -> BackupSystemDatabaseReport {
        let common = &self.roots.aliases.current.common;
        BackupSystemDatabaseReport {
            installation_hash: selected.hash().to_owned(),
            system_database_hash: common.system_database_hash.clone(),
            system_database_bytes: common.system_database_bytes,
            retained_old_files: selected.retained_files(),
            roots: self.roots,
        }
    }
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    quiescent: &QuiescentIndex,
    stop: &AtomicBool,
) -> Result<BackupSystemDatabaseReport> {
    quiescent.require_root(context.system)?;
    let before = Plan::capture(context, prepared, decision, expected, stop)?;
    let complete = preparation_path_exists(context.system, &context.system.path().join(COMPLETE))?;
    // Completion is never downgraded into a repair request. A missing old set,
    // changed new DB, sidecar or missing selector must remain a concrete fault.
    let selected = if complete {
        let selected = Selected::read(context.system, &before)?;
        selected.require_complete(context.system, &before, stop)?;
        selected
    } else {
        before.require_capacity(context.system)?;
        let selected = Selected::capture(context.system, &before, stop)?;
        quiescent.require_root(context.system)?;
        context.sources.recheck(context.system)?;
        selected.preserve_old(context.system, &before, stop)?;
        selected.require_retained(context.system, &before, stop)?;
        preparation_checkpoint(stop)?;
        // No replace: an unexpected ordinary name cannot be overwritten. Every
        // old sidecar and main file has already moved under its exact stamp.
        install_preparation_bytes(
            context.system,
            &context.system.path().join("index.sqlite"),
            &before.database,
        )?;
        selected.require_installed(context.system, &before, stop)?;
        selected
    };
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if before.binding_hash != after.binding_hash {
        return Err(invalid(
            "SYSTEM native installation proof changed during publication",
        ));
    }
    quiescent.require_root(context.system)?;
    selected.require_installed(context.system, &after, stop)?;
    install_preparation_bytes(
        context.system,
        &context.system.path().join(COMPLETE),
        selected.bytes(),
    )?;
    selected.require_complete(context.system, &after, stop)?;
    context.sources.recheck(context.system)?;
    Ok(after.report(&selected))
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupSystemDatabaseReport> {
    let before = Plan::capture(context, prepared, decision, expected, stop)?;
    let selected = Selected::read(context.system, &before)?;
    selected.require_complete(context.system, &before, stop)?;
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if before.binding_hash != after.binding_hash {
        return Err(invalid(
            "SYSTEM native installation proof changed during observation",
        ));
    }
    selected.require_complete(context.system, &after, stop)?;
    Ok(after.report(&selected))
}
