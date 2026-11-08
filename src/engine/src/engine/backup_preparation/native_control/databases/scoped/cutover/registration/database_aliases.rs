//! Generation-bound database aliases, selected only by the exact original
//! task and its actual complete Publication. No pathname from a stored alias or
//! request is trusted. These names do not replace normal Engine database paths.
use super::*;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

mod read;
pub(super) use read::read as read_range;
pub use read::{BackupDatabaseRange, BackupDatabaseRead};

const INTENT: &str = "backup-preparation/published-database-aliases.json";
const RECORD: &str = "backup-published-database-aliases.json";
const MAX_DESCRIPTOR: usize = 128 * 1024;
const MAX_SYSTEM: u64 = 64 * 1024 * 1024;
const MAX_DISK: u64 = 32 * 1024 * 1024;
const MARGIN: u64 = 1024 * 1024;

/// Identity, never a root supplied by the caller. Original and target disks are
/// intentionally distinct scopes even if they have similar names or contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serialization(tag = "scope", content = "identity", rename_all = "kebab-case")]
pub enum BackupDatabaseScope {
    System,
    OriginalDisk(DiskRegistrationIdentity),
    TargetDisk(DiskRegistrationIdentity),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "kebab-case")]
pub enum BackupDatabaseFile {
    SystemCurrent,
    DiskCurrent,
    DiskHistory,
}
impl BackupDatabaseFile {
    fn maximum(self) -> u64 {
        match self {
            Self::SystemCurrent => MAX_SYSTEM,
            Self::DiskCurrent | Self::DiskHistory => MAX_DISK,
        }
    }
}

/// Diagnostic description of an actual generation-local database. The physical
/// name is intentionally private: an API caller selects only scope + file kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupDatabaseAlias {
    pub scope: BackupDatabaseScope,
    pub file: BackupDatabaseFile,
    #[serialization(with = "storage_contract::u64_string")]
    pub bytes: u64,
    pub digest: String,
    relative_file: String,
    root: PhysicalBinding,
}
#[derive(Clone, Debug)]
pub struct BackupDatabaseAliasesReport {
    pub current: BackupCurrentReadReport,
    pub aliases_hash: String,
    pub aliases: Vec<BackupDatabaseAlias>,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    common_manifest_hash: &'a str,
    publication_hash: &'a str,
    aliases: &'a [BackupDatabaseAlias],
}
struct Plan {
    report: BackupDatabaseAliasesReport,
    bytes: Vec<u8>,
}

fn require_named(root: &DiskRoot, expected: &PhysicalBinding) -> Result<()> {
    if PhysicalBinding::of(root)? != *expected
        || PhysicalBinding::of(&DiskRoot::open(root.path())?)? != *expected
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "Backup database alias root differs from its held and named directory",
        ));
    }
    Ok(())
}

/// Every path and digest below comes from a fully reconstructed common plan.
/// Stored descriptors are compared byte-for-byte, never deserialized as I/O.
fn visit_files(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    current: &BackupCurrentReadReport,
    stop: &AtomicBool,
    mut action: impl FnMut(&DiskRoot, BackupDatabaseScope, BackupDatabaseFile, &str, &str) -> Result<()>,
) -> Result<()> {
    context.sources.recheck(context.system)?;
    action(
        context.system,
        BackupDatabaseScope::System,
        BackupDatabaseFile::SystemCurrent,
        "backup-common-databases/index.sqlite",
        &current.common.system_database_hash,
    )?;
    context.sources.for_each(
        stop,
        |root, identity| {
            let actual = current
                .common
                .original_disks
                .iter()
                .find(|disk| &disk.identity == identity)
                .ok_or_else(|| invalid("Backup alias omits an original disk"))?;
            for (file, name, hash) in [
                (
                    BackupDatabaseFile::DiskCurrent,
                    "backup-common-databases/.zxs-disk.sqlite",
                    actual.current_hash.as_str(),
                ),
                (
                    BackupDatabaseFile::DiskHistory,
                    "backup-common-databases/.zxs-disk-history.sqlite",
                    actual.history_hash.as_str(),
                ),
            ] {
                action(
                    root,
                    BackupDatabaseScope::OriginalDisk(identity.clone()),
                    file,
                    name,
                    hash,
                )?;
            }
            Ok(())
        },
        &prepared.task.selection().first().pool_id,
    )?;
    for (index, target) in prepared.targets.iter().enumerate() {
        preparation_checkpoint(stop)?;
        let identity = &prepared.projection.targets[index].node.identity;
        let actual = current
            .common
            .published
            .targets
            .iter()
            .find(|disk| &disk.target == identity)
            .ok_or_else(|| invalid("Backup alias omits a selected target"))?;
        named_target(&target.root, &prepared.task.selection().jobs()[index])?;
        for (file, name, hash) in [
            (
                BackupDatabaseFile::DiskCurrent,
                ".zxs-disk.sqlite",
                actual.disk_current_hash.as_str(),
            ),
            (
                BackupDatabaseFile::DiskHistory,
                ".zxs-disk-history.sqlite",
                actual.disk_history_hash.as_str(),
            ),
        ] {
            action(
                &target.root,
                BackupDatabaseScope::TargetDisk(identity.clone()),
                file,
                name,
                hash,
            )?;
        }
        named_target(&target.root, &prepared.task.selection().jobs()[index])?;
    }
    context.sources.recheck(context.system)
}
fn visit_roots(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    stop: &AtomicBool,
    mut action: impl FnMut(&DiskRoot) -> Result<()>,
) -> Result<()> {
    context.sources.recheck(context.system)?;
    action(context.system)?;
    context.sources.for_each(
        stop,
        |root, _| action(root),
        &prepared.task.selection().first().pool_id,
    )?;
    for (index, target) in prepared.targets.iter().enumerate() {
        preparation_checkpoint(stop)?;
        named_target(&target.root, &prepared.task.selection().jobs()[index])?;
        action(&target.root)?;
        named_target(&target.root, &prepared.task.selection().jobs()[index])?;
    }
    context.sources.recheck(context.system)
}
impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let current = current_read::observe(context, prepared, decision, expected, stop)?;
        let mut aliases = Vec::new();
        visit_files(
            context,
            prepared,
            &current,
            stop,
            |root, scope, file, relative, digest| {
                if aliases.len() == 81 {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "Backup alias set exceeds 81 databases",
                    ));
                }
                let physical = PhysicalBinding::of(root)?;
                require_named(root, &physical)?;
                let bytes =
                    read::verify_range(root, relative, file.maximum(), digest, None, stop)?.0;
                aliases.push(BackupDatabaseAlias {
                    scope,
                    file,
                    bytes,
                    digest: digest.into(),
                    relative_file: relative.into(),
                    root: physical.clone(),
                });
                require_named(root, &physical)
            },
        )?;
        let bytes = canonical_json(&Manifest {
            format: "zxs-backup-published-database-aliases",
            source_fence_hash: prepared.source_fence_hash(),
            common_manifest_hash: &current.common.common_manifest_hash,
            publication_hash: expected.as_str(),
            aliases: &aliases,
        })?;
        if bytes.len() > MAX_DESCRIPTOR {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup alias descriptor exceeds 128 KiB",
            ));
        }
        Ok(Self {
            report: BackupDatabaseAliasesReport {
                current,
                aliases_hash: blake3::hash(&bytes).to_hex().to_string(),
                aliases,
            },
            bytes,
        })
    }
    fn require_record(&self, root: &DiskRoot, name: &str) -> Result<()> {
        if root.read_bytes(&root.path().join(name), MAX_DESCRIPTOR)? != self.bytes {
            return Err(invalid(
                "Backup database aliases differ from the original complete selection",
            ));
        }
        Ok(())
    }
    fn require_complete(
        &self,
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_record(context.system, INTENT)?;
        visit_roots(context, prepared, stop, |root| {
            self.require_record(root, RECORD)
        })
    }
    fn require_same(&self, next: &Self) -> Result<()> {
        if self.bytes != next.bytes {
            return Err(invalid(
                "Backup database alias selection changed during observation",
            ));
        }
        Ok(())
    }
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupDatabaseAliasesReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    // There is no partially usable set. If any durable per-root selector exists,
    // the exact SYSTEM intent must exist too. Never infer a new choice from it.
    let mut selected = false;
    visit_roots(context, prepared, stop, |root| {
        if preparation_path_exists(root, &root.path().join(RECORD))? {
            plan.require_record(root, RECORD)?;
            selected = true;
        }
        if root.available()? < MARGIN + 2 * plan.bytes.len() as u64 {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "Backup alias record lacks safe staging capacity",
            ));
        }
        Ok(())
    })?;
    if selected || preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
        plan.require_record(context.system, INTENT)?;
    }
    install_preparation_bytes(
        context.system,
        &context.system.path().join(INTENT),
        &plan.bytes,
    )?;
    visit_roots(context, prepared, stop, |root| {
        let physical = PhysicalBinding::of(root)?;
        require_named(root, &physical)?;
        install_preparation_bytes(root, &root.path().join(RECORD), &plan.bytes)?;
        plan.require_record(root, RECORD)?;
        require_named(root, &physical)
    })?;
    let final_plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_same(&final_plan)?;
    final_plan.require_complete(context, prepared, stop)?;
    Ok(final_plan.report)
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupDatabaseAliasesReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_complete(context, prepared, stop)?;
    Ok(plan.report)
}
