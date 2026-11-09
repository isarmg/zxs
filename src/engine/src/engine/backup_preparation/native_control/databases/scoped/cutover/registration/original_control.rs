//! Original roots receive independent native control material beside their
//! actual reserved first journal. No selector, journal, seal or vote is rewritten.
use super::*;
use crate::engine::membership_transition::{
    AdmittedBackupDecision, BackupFirstPublicationHash, PreparedBackupFirstCandidate,
    PreparedBackupFirstPublication, pending::activation::PreparedBackupTargetLocator,
};
use target_locator::store::{Inventory, Presence};

const INTENT: &str = "backup-preparation/original-generation-control.json";
const SELECTED: &str = "backup-original-generation-selected.json";
const LOCAL_COMPLETE: &str = "backup-original-generation-installed.json";
const COMPLETE: &str = "backup-original-generation-set-installed.json";
const MAX_RECORD: usize = 64 * 1024;
const MAX_LOCAL: usize = 8 * 1024;
// Before a NEW irreversible choice: full bounded material, largest possible
// archive file for atomic staging, per-file allocation and completion margin.
// Existing choices use only actual missing-file accounting, never this bound.
pub(super) const PRE_CHOICE_BYTES: u64 =
    512 * 1024 * 1024 + 256 * 1024 * 1024 + 65_536 * 8192 + 1024 * 1024;

/// Actual original first-slot file, never a synthesized journal recipe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupOriginalJournalReport {
    pub bytes_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub inode: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupOriginalControlMemberReport {
    pub identity: DiskRegistrationIdentity,
    #[serialization(with = "storage_contract::u64_string")]
    pub root_device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub root_inode: u64,
    pub original_seal_hash: String,
    pub original_anchor_hash: String,
    pub journal: Option<BackupOriginalJournalReport>,
}

/// Independent original-root recovery files, not active pool selection.
#[derive(Clone, Debug)]
pub struct BackupOriginalControlReport {
    pub targets: BackupTargetLocatorReport,
    pub installation_hash: String,
    pub originals: Vec<BackupOriginalControlMemberReport>,
    pub files_per_original: usize,
    pub bytes_per_original: u64,
}

#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    target_locator_installation: &'a str,
    authority_hash: &'a str,
    archive_hash: &'a str,
    files_hash: &'a str,
    files_per_original: usize,
    bytes_per_original: u64,
    originals: &'a [BackupOriginalControlMemberReport],
}

#[derive(Serialize)]
struct Selection<'a> {
    format: &'static str,
    plan_hash: &'a str,
    original: &'a BackupOriginalControlMemberReport,
}

struct Plan {
    targets: BackupTargetLocatorReport,
    material: PreparedBackupTargetLocator,
    candidate: PreparedBackupFirstCandidate,
    publication: PreparedBackupFirstPublication,
    inventory: Inventory,
    originals: Vec<BackupOriginalControlMemberReport>,
    bytes: Vec<u8>,
    hash: String,
}

impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let targets = target_locator::observe(context, prepared, decision, expected, stop)?;
        let (_, candidate) = first_candidate::read_verified(prepared, decision, stop)?;
        let publication = context
            .choices
            .read_first_publication(prepared, &candidate, stop)?;
        let admitted = candidate.writer_configuration(
            &publication,
            generation_control::retained_archive(prepared, 0)?,
            stop,
        )?;
        let inputs = generation_inputs::observe(prepared, decision, stop)?;
        let (control, _) =
            generation_control::read_authority_input(prepared, decision, &inputs, 0, stop)?;
        let material = admitted.target_locator(control, stop)?;
        if material.authority_hash() != targets.authority_hash
            || material.archive_hash() != targets.archive_hash
        {
            return Err(invalid(
                "original Backup control changes its complete target authority or archive",
            ));
        }
        let inventory = Inventory::capture(material.original_files())?;
        let mut plan = Self {
            targets,
            material,
            candidate,
            publication,
            inventory,
            originals: Vec::new(),
            bytes: Vec::new(),
            hash: String::new(),
        };
        context.sources.for_each_disk(
            stop,
            |root, identity| {
                let observed = plan.observe_member(root, identity, stop)?;
                plan.originals.push(observed);
                Ok(())
            },
            &prepared.task.selection().first().pool_id,
        )?;
        plan.bytes = canonical_json(&Manifest {
            format: "zxs-backup-original-generation-control",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: decision.hash().as_str(),
            publication_hash: expected.as_str(),
            target_locator_installation: &plan.targets.installation_hash,
            authority_hash: plan.material.authority_hash(),
            archive_hash: plan.material.archive_hash(),
            files_hash: &plan.inventory.hash,
            files_per_original: plan.inventory.count(),
            bytes_per_original: plan.inventory.bytes,
            originals: &plan.originals,
        })?;
        if plan.bytes.len() > MAX_RECORD {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "original Backup control plan exceeds 64 KiB",
            ));
        }
        plan.hash = blake3::hash(&plan.bytes).to_hex().to_string();
        Ok(plan)
    }

    fn observe_member(
        &self,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        stop: &AtomicBool,
    ) -> Result<BackupOriginalControlMemberReport> {
        let disk = self.material.original_disk(identity)?;
        let physical = PhysicalBinding::of(root)?;
        let seal = root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?;
        let anchor = root.read_bytes(
            &root.path().join("metadata/pool-membership/config.json"),
            MAX_METADATA,
        )?;
        self.candidate
            .original_config()
            .verify_saved_metadata_membership(&decode_strict(&anchor)?)?;
        let journal = if disk.metadata_member {
            let (physical, bytes_hash) =
                self.publication
                    .original_journal_binding(root, disk, &self.candidate, stop)?;
            Some(BackupOriginalJournalReport {
                bytes_hash,
                device: physical.device,
                inode: physical.inode,
            })
        } else {
            let journal =
                crate::engine::membership_transition::pending::activation::generation_directory(
                    disk,
                    self.material.authority_hash(),
                )
                .join("journal.bin");
            if preparation_path_exists(root, &journal)? {
                return Err(invalid(
                    "original data-only Backup root cannot acquire a metadata journal",
                ));
            }
            None
        };
        Ok(BackupOriginalControlMemberReport {
            identity: identity.clone(),
            root_device: physical.device,
            root_inode: physical.inode,
            original_seal_hash: blake3::hash(&seal).to_hex().to_string(),
            original_anchor_hash: blake3::hash(&anchor).to_hex().to_string(),
            journal,
        })
    }

    fn member(
        &self,
        identity: &DiskRegistrationIdentity,
    ) -> Result<&BackupOriginalControlMemberReport> {
        self.originals
            .iter()
            .find(|member| &member.identity == identity)
            .ok_or_else(|| {
                invalid("original control participant is absent from the exact selected set")
            })
    }

    fn selection(&self, identity: &DiskRegistrationIdentity) -> Result<Vec<u8>> {
        let bytes = canonical_json(&Selection {
            format: "zxs-backup-original-generation-selection",
            plan_hash: &self.hash,
            original: self.member(identity)?,
        })?;
        if bytes.len() > MAX_LOCAL {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "original Backup control selection exceeds 8 KiB",
            ));
        }
        Ok(bytes)
    }

    fn require_local(
        &self,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        name: &str,
    ) -> Result<()> {
        if root.read_bytes(&root.path().join(name), MAX_LOCAL)? != self.selection(identity)? {
            return Err(invalid(
                "original Backup control changed its selected plan or exact physical resources",
            ));
        }
        Ok(())
    }

    fn inspect(
        &self,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        stop: &AtomicBool,
    ) -> Result<()> {
        let member = self.member(identity)?;
        if &self.observe_member(root, identity, stop)? != member {
            return Err(invalid(
                "original Backup root, seal, anchor or published journal changed",
            ));
        }
        let existing = if member.journal.is_some() {
            vec![self.material.original_scopes()[1].join("journal.bin")]
        } else {
            Vec::new()
        };
        self.inventory
            .inspect_under(root, &self.material.original_scopes(), &existing, stop)
    }

    fn preflight(
        &self,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.inspect(root, identity, stop)?;
        if preparation_path_exists(root, &root.path().join(SELECTED))? {
            self.require_local(root, identity, SELECTED)?;
        } else {
            self.inventory.require_absent_files(root, stop)?;
            if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
                return Err(invalid(
                    "original Backup control completion lost its prior selection",
                ));
            }
        }
        if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
            self.verify(root, identity, stop)?;
        } else {
            self.inventory
                .verify_files(root, Presence::Existing, stop)?;
        }
        Ok(())
    }

    fn verify_material(
        &self,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.inspect(root, identity, stop)?;
        self.inventory
            .verify_files(root, Presence::Complete, stop)?;
        self.material.verify_original_archive(root, identity, stop)
    }

    fn verify(
        &self,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_local(root, identity, SELECTED)?;
        self.require_local(root, identity, LOCAL_COMPLETE)?;
        self.verify_material(root, identity, stop)
    }

    fn require_record(&self, root: &DiskRoot, path: &str) -> Result<()> {
        if root.read_bytes(&root.path().join(path), MAX_RECORD)? != self.bytes {
            return Err(invalid(
                "original Backup control lost its exact all-participant plan",
            ));
        }
        Ok(())
    }

    fn report(self) -> BackupOriginalControlReport {
        BackupOriginalControlReport {
            targets: self.targets,
            installation_hash: self.hash,
            originals: self.originals,
            files_per_original: self.inventory.count(),
            bytes_per_original: self.inventory.bytes,
        }
    }
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupOriginalControlReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    let pool = &prepared.task.selection().first().pool_id;
    let mut completed = false;
    target_writers::visit(context, prepared, stop, |root| {
        if preparation_path_exists(root, &root.path().join(COMPLETE))? {
            plan.require_record(root, COMPLETE)?;
            completed = true;
        }
        Ok(())
    })?;
    if completed {
        plan.require_record(context.system, INTENT)?;
        context.sources.for_each_disk(
            stop,
            |root, identity| plan.verify(root, identity, stop),
            pool,
        )?;
    } else {
        if preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
            plan.require_record(context.system, INTENT)?;
        }
        let reserve = context.sources.metadata_reserve_bytes();
        let minimum = reserve.checked_add(1024 * 1024).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "original Backup control reserve overflow",
            )
        })?;
        target_writers::visit(context, prepared, stop, |root| {
            if root.available()? < minimum {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "original Backup control requires reserve plus 1 MiB on every participant",
                ));
            }
            Ok(())
        })?;
        context.sources.for_each_disk(
            stop,
            |root, identity| {
                plan.preflight(root, identity, stop)?;
                plan.inventory.require_capacity(root, reserve)
            },
            pool,
        )?;
        install_preparation_bytes(
            context.system,
            &context.system.path().join(INTENT),
            &plan.bytes,
        )?;
        context.sources.for_each_disk(
            stop,
            |root, identity| {
                plan.preflight(root, identity, stop)?;
                install_preparation_bytes(
                    root,
                    &root.path().join(SELECTED),
                    &plan.selection(identity)?,
                )
            },
            pool,
        )?;
        context.sources.for_each_disk(
            stop,
            |root, identity| {
                plan.preflight(root, identity, stop)?;
                if !preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
                    for (path, bytes) in plan.material.original_files() {
                        preparation_checkpoint(stop)?;
                        install_preparation_bytes(root, &root.path().join(path), bytes)?;
                    }
                }
                plan.verify_material(root, identity, stop)?;
                install_preparation_bytes(
                    root,
                    &root.path().join(LOCAL_COMPLETE),
                    &plan.selection(identity)?,
                )
            },
            pool,
        )?;
    }
    let original = plan.bytes.clone();
    drop(plan);
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if after.bytes != original {
        return Err(invalid(
            "original Backup control inputs changed before common completion",
        ));
    }
    after.require_record(context.system, INTENT)?;
    context.sources.for_each_disk(
        stop,
        |root, identity| after.verify(root, identity, stop),
        pool,
    )?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        target.with_writer_io(&after.targets.seals.io.admission_hash, |root| {
            install_preparation_bytes(root, &root.path().join(COMPLETE), &after.bytes)
        })?;
    }
    context.sources.for_each_disk(
        stop,
        |root, _| install_preparation_bytes(root, &root.path().join(COMPLETE), &after.bytes),
        pool,
    )?;
    install_preparation_bytes(
        context.system,
        &context.system.path().join(COMPLETE),
        &after.bytes,
    )?;
    target_writers::visit(context, prepared, stop, |root| {
        after.require_record(root, COMPLETE)
    })?;
    context.sources.recheck(context.system)?;
    prepared.recheck_names()?;
    Ok(after.report())
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupOriginalControlReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_record(context.system, INTENT)?;
    context.sources.for_each_disk(
        stop,
        |root, identity| plan.verify(root, identity, stop),
        &prepared.task.selection().first().pool_id,
    )?;
    target_writers::visit(context, prepared, stop, |root| {
        plan.require_record(root, COMPLETE)
    })?;
    Ok(plan.report())
}
