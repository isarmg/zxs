//! Actual ordinary disk-root names, installed under the original all-member
//! decision while the source remains stopped. The independent SYSTEM index
//! connection and active Engine Current/writer are deliberately not changed.
use super::*;
use crate::database_install::{DISK_DATABASE_NAME, DISK_HISTORY_DATABASE_NAME, InstallAdmission};
use crate::engine::disk_catalog::root_install::{
    BACKUP_SELECTION, PreparedRootDatabases, require_published_pair,
};
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

const INTENT: &str = "backup-preparation/root-databases.json";
const COMPLETE: &str = "backup-root-databases-installed.json";
const MAX_DESCRIPTOR: usize = 128 * 1024;
const MAX_DATABASE: usize = 32 * 1024 * 1024;
const MARGIN: u64 = 1024 * 1024;

/// All ordinary disk-root pairs have been freshly compared with the actual
/// published common set. No field is an active writer or source-release permit.
#[derive(Clone, Debug)]
pub struct BackupRootDatabasesReport {
    pub aliases: BackupDatabaseAliasesReport,
    pub installation_hash: String,
    pub original_disks: Vec<DiskRegistrationIdentity>,
    pub selected_targets: usize,
}

#[derive(Serialize)]
struct OriginalBinding {
    identity: DiskRegistrationIdentity,
    physical: PhysicalBinding,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    publication_hash: &'a str,
    aliases_hash: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    original_roots: &'a [OriginalBinding],
    original_databases: &'a [BackupOriginalDatabaseReport],
}
struct Plan {
    report: BackupRootDatabasesReport,
    bindings: Vec<OriginalBinding>,
    bytes: Vec<u8>,
    sequence: u64,
    reserve: u64,
}
fn require_name(root: &DiskRoot, physical: &PhysicalBinding) -> Result<()> {
    if PhysicalBinding::of(root)? != *physical
        || PhysicalBinding::of(&DiskRoot::open(root.path())?)? != *physical
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "ordinary Backup database root no longer names its original held directory",
        ));
    }
    Ok(())
}

impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        // This observes every original vote and every actual target image,
        // database and namespace. A persisted progress/selector is not input.
        let aliases = database_aliases::observe(context, prepared, decision, expected, stop)?;
        let (_, candidate) = first_candidate::read_verified(prepared, decision, stop)?;
        context
            .choices
            .observe_first_publication(prepared, &candidate, stop)?;
        let sequence = candidate.sequence();
        let reserve = candidate.original_config().metadata_reserve_bytes;
        let mut bindings = Vec::new();
        context.sources.for_each_disk(
            stop,
            |root, identity| {
                let physical = PhysicalBinding::of(root)?;
                require_name(root, &physical)?;
                bindings.push(OriginalBinding {
                    identity: identity.clone(),
                    physical,
                });
                Ok(())
            },
            &prepared.task.selection().first().pool_id,
        )?;
        let bytes = canonical_json(&Manifest {
            format: "zxs-backup-ordinary-root-databases",
            source_fence_hash: prepared.source_fence_hash(),
            publication_hash: expected.as_str(),
            aliases_hash: &aliases.aliases_hash,
            sequence,
            original_roots: &bindings,
            original_databases: &aliases.current.common.original_disks,
        })?;
        if bytes.len() > MAX_DESCRIPTOR {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup root database selection exceeds 128 KiB",
            ));
        }
        Ok(Self {
            report: BackupRootDatabasesReport {
                installation_hash: blake3::hash(&bytes).to_hex().to_string(),
                original_disks: bindings
                    .iter()
                    .map(|binding| binding.identity.clone())
                    .collect(),
                selected_targets: prepared.targets.len(),
                aliases,
            },
            bindings,
            bytes,
            sequence,
            reserve,
        })
    }
    fn require_record(&self, root: &DiskRoot, name: &str) -> Result<()> {
        if root.read_bytes(&root.path().join(name), MAX_DESCRIPTOR)? != self.bytes {
            return Err(invalid(
                "ordinary database selection differs from this complete original publication",
            ));
        }
        Ok(())
    }
    fn visit_roots(
        &self,
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
        mut action: impl FnMut(&DiskRoot) -> Result<()>,
    ) -> Result<()> {
        context.sources.recheck(context.system)?;
        action(context.system)?;
        context.sources.for_each_disk(
            stop,
            |root, identity| {
                let binding = self
                    .bindings
                    .iter()
                    .find(|binding| &binding.identity == identity)
                    .ok_or_else(|| {
                        invalid("ordinary database selection omits an original binding")
                    })?;
                require_name(root, &binding.physical)?;
                action(root)?;
                require_name(root, &binding.physical)
            },
            &prepared.task.selection().first().pool_id,
        )?;
        for (index, target) in prepared.targets.iter().enumerate() {
            preparation_checkpoint(stop)?;
            let job = &prepared.task.selection().jobs()[index];
            named_target(&target.root, job)?;
            action(&target.root)?;
            named_target(&target.root, job)?;
        }
        context.sources.recheck(context.system)
    }
    fn visit_pairs(
        &self,
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
        mut action: impl FnMut(&DiskRoot, &DiskRegistrationIdentity, [Vec<u8>; 2]) -> Result<()>,
    ) -> Result<()> {
        context.sources.for_each_disk(
            stop,
            |root, identity| {
                let binding = self
                    .bindings
                    .iter()
                    .find(|binding| &binding.identity == identity)
                    .ok_or_else(|| invalid("root database pair lacks its original binding"))?;
                require_name(root, &binding.physical)?;
                let expected = self
                    .report
                    .aliases
                    .current
                    .common
                    .original_disks
                    .iter()
                    .find(|disk| &disk.identity == identity)
                    .ok_or_else(|| invalid("root database pair lacks its published scope"))?;
                let read = |name: &str, hash: &str| -> Result<Vec<u8>> {
                    preparation_checkpoint(stop)?;
                    let bytes = root.read_bytes(
                        &root
                            .path()
                            .join("metadata/backup-common-databases")
                            .join(name),
                        MAX_DATABASE,
                    )?;
                    if blake3::hash(&bytes).to_hex().as_str() != hash {
                        return Err(invalid(
                            "ordinary database source differs from its exact published copy",
                        ));
                    }
                    Ok(bytes)
                };
                let pair = [
                    read(DISK_DATABASE_NAME, &expected.current_hash)?,
                    read(DISK_HISTORY_DATABASE_NAME, &expected.history_hash)?,
                ];
                action(root, identity, pair)?;
                require_name(root, &binding.physical)
            },
            &prepared.task.selection().first().pool_id,
        )
    }
    fn require_pairs(
        &self,
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.visit_pairs(context, prepared, stop, |root, identity, pair| {
            require_published_pair(root, identity, &pair, stop)
        })
    }
    fn require_complete(
        &self,
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_record(context.system, INTENT)?;
        self.visit_roots(context, prepared, stop, |root| {
            self.require_record(root, BACKUP_SELECTION)?;
            self.require_record(root, COMPLETE)
        })?;
        self.require_pairs(context, prepared, stop)
    }
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupRootDatabasesReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    let has_intent = preparation_path_exists(context.system, &context.system.path().join(INTENT))?;
    let mut started = false;
    let mut completed = false;
    plan.visit_roots(context, prepared, stop, |root| {
        for (name, present) in [(BACKUP_SELECTION, &mut started), (COMPLETE, &mut completed)] {
            if preparation_path_exists(root, &root.path().join(name))? {
                plan.require_record(root, name)?;
                *present = true;
            }
        }
        if root.available()? < MARGIN + 4 * plan.bytes.len() as u64 {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "ordinary database selectors lack safe staging space",
            ));
        }
        Ok(())
    })?;
    if has_intent || started || completed {
        plan.require_record(context.system, INTENT)?;
    }
    // A partial distribution of COMPLETE already claims the whole pair set.
    // It must not permit replacement of a missing/corrupt claimed-current file.
    if completed {
        plan.visit_roots(context, prepared, stop, |root| {
            plan.require_record(root, BACKUP_SELECTION)
        })?;
        plan.require_pairs(context, prepared, stop)?;
    }
    let admission = if has_intent {
        InstallAdmission::DecidedRecovery
    } else {
        InstallAdmission::NewOperation
    };
    plan.visit_pairs(context, prepared, stop, |root, identity, pair| {
        let captured =
            PreparedRootDatabases::capture(root, identity, pair, plan.sequence, admission)?;
        captured.require_capacity(root, admission, plan.reserve)
    })?;
    // The SYSTEM record precedes every ordinary name exchange. Every original
    // disk gets its refusal marker before any pair is installed, so a process
    // death cannot let old-State cache maintenance overwrite a partial switch.
    install_preparation_bytes(
        context.system,
        &context.system.path().join(INTENT),
        &plan.bytes,
    )?;
    plan.visit_roots(context, prepared, stop, |root| {
        install_preparation_bytes(root, &root.path().join(BACKUP_SELECTION), &plan.bytes)?;
        plan.require_record(root, BACKUP_SELECTION)
    })?;
    if !completed {
        plan.visit_pairs(context, prepared, stop, |root, identity, pair| {
            plan.require_record(root, BACKUP_SELECTION)?;
            let captured = PreparedRootDatabases::capture(
                root,
                identity,
                pair,
                plan.sequence,
                InstallAdmission::DecidedRecovery,
            )?;
            captured.require_capacity(root, InstallAdmission::DecidedRecovery, plan.reserve)?;
            captured.apply(root, stop)
        })?;
    }
    plan.require_pairs(context, prepared, stop)?;
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if after.bytes != plan.bytes {
        return Err(invalid(
            "ordinary root database proof changed during installation",
        ));
    }
    after.visit_roots(context, prepared, stop, |root| {
        after.require_record(root, BACKUP_SELECTION)?;
        install_preparation_bytes(root, &root.path().join(COMPLETE), &after.bytes)?;
        after.require_record(root, COMPLETE)
    })?;
    after.require_complete(context, prepared, stop)?;
    Ok(after.report)
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupRootDatabasesReport> {
    let before = Plan::capture(context, prepared, decision, expected, stop)?;
    before.require_complete(context, prepared, stop)?;
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if before.bytes != after.bytes {
        return Err(invalid(
            "ordinary root database proof changed during observation",
        ));
    }
    after.require_complete(context, prepared, stop)?;
    Ok(after.report)
}
