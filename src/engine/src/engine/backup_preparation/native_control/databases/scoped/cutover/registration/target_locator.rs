//! Ordinary target-only native generation locators and independent archives.
//! Original member selectors, journals, Engine Current and fences stay intact.
use super::*;
use crate::engine::membership_transition::pending::activation::PreparedBackupTargetLocator;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

pub(super) mod store;
pub(super) use store::{is_namespace, require_staging};

const INTENT: &str = "backup-preparation/target-generation-locators.json";
const COMPLETE: &str = "backup-target-generation-set-installed.json";
const MAX_RECORD: usize = 64 * 1024;

/// Ordinary-address cold target evidence, not the active pool generation.
#[derive(Clone, Debug)]
pub struct BackupTargetLocatorReport {
    pub seals: BackupTargetSealsReport,
    pub installation_hash: String,
    pub authority_hash: String,
    pub archive_hash: String,
    pub files_per_target: usize,
    pub bytes_per_target: u64,
    pub targets: usize,
}

#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    seal_installation_hash: &'a str,
    authority_hash: &'a str,
    transition_hash: &'a str,
    archive_hash: &'a str,
    files_hash: &'a str,
    files_per_target: usize,
    bytes_per_target: u64,
    targets: &'a [BackupTargetSealReport],
}

struct Plan {
    seals: BackupTargetSealsReport,
    candidate: crate::engine::membership_transition::PreparedBackupFirstCandidate,
    material: PreparedBackupTargetLocator,
    inventory: store::Inventory,
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
        let seals = target_seals::observe(context, prepared, decision, expected, stop)?;
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
        let inventory = store::Inventory::capture(material.files())?;
        if material.authority_hash() != seals.io.writers.native.authority_hash
            || material.bytes() as u64 != inventory.bytes
            || publication.report().publication_hash != expected.as_str()
        {
            return Err(invalid(
                "Backup target locator changes the original native writer or publication",
            ));
        }
        let bytes = canonical_json(&Manifest {
            format: "zxs-backup-target-generation-locators",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: decision.hash().as_str(),
            publication_hash: expected.as_str(),
            seal_installation_hash: &seals.installation_hash,
            authority_hash: material.authority_hash(),
            transition_hash: material.transition_hash(),
            archive_hash: material.archive_hash(),
            files_hash: &inventory.hash,
            files_per_target: inventory.count(),
            bytes_per_target: inventory.bytes,
            targets: &seals.targets,
        })?;
        if bytes.len() > MAX_RECORD {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup locator descriptor exceeds 64 KiB",
            ));
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        Ok(Self {
            seals,
            candidate,
            material,
            inventory,
            bytes,
            hash,
        })
    }

    fn local(&self, root: &DiskRoot) -> Result<store::LocalSelection> {
        store::LocalSelection::capture(
            root,
            &store::SelectionInputs {
                plan_hash: &self.hash,
                native_installation_hash: &self.seals.io.writers.native.installation_hash,
                authority_hash: self.material.authority_hash(),
                transition_hash: self.material.transition_hash(),
                archive_hash: self.material.archive_hash(),
                files_hash: &self.inventory.hash,
            },
        )
    }

    fn require_record(&self, root: &DiskRoot, name: &str) -> Result<()> {
        if root.read_bytes(&root.path().join(name), MAX_RECORD)? != self.bytes {
            return Err(invalid(
                "Backup target locator lost its exact original all-target plan",
            ));
        }
        Ok(())
    }

    fn verify_target(&self, root: &DiskRoot, disk: &str, stop: &AtomicBool) -> Result<()> {
        self.inventory
            .verify_present(root, store::Presence::Complete, stop)?;
        self.material
            .verify_reader(root, disk, self.candidate.candidate(), stop)
    }

    fn verify_targets(&self, prepared: &PreparedBackupRegistry, stop: &AtomicBool) -> Result<()> {
        for (target, selected) in prepared.targets.iter().zip(&self.seals.targets) {
            preparation_checkpoint(stop)?;
            target.with_writer_io(&self.seals.io.admission_hash, |root| {
                self.local(root)?.require_complete(root)?;
                self.verify_target(root, &selected.identity.disk_id, stop)
            })?;
        }
        prepared.recheck_names()
    }

    fn report(self) -> BackupTargetLocatorReport {
        BackupTargetLocatorReport {
            targets: self.seals.targets.len(),
            authority_hash: self.material.authority_hash().into(),
            archive_hash: self.material.archive_hash().into(),
            files_per_target: self.inventory.count(),
            bytes_per_target: self.inventory.bytes,
            installation_hash: self.hash,
            seals: self.seals,
        }
    }
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetLocatorReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    let mut complete = false;
    target_writers::visit(context, prepared, stop, |root| {
        if preparation_path_exists(root, &root.path().join(COMPLETE))? {
            plan.require_record(root, COMPLETE)?;
            complete = true;
        }
        Ok(())
    })?;
    if complete {
        plan.require_record(context.system, INTENT)?;
        plan.verify_targets(prepared, stop)?;
    } else {
        if preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
            plan.require_record(context.system, INTENT)?;
        }
        let reserve = context.sources.metadata_reserve_bytes();
        let required = reserve.checked_add(1024 * 1024).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "Backup target locator reserve overflow",
            )
        })?;
        target_writers::visit(context, prepared, stop, |root| {
            if root.available()? < required {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "Backup locator completion requires the original reserve plus 1 MiB on every participant",
                ));
            }
            Ok(())
        })?;
        // All destinations and existing records are checked before selecting
        // even the first ordinary namespace. A local completion forbids repair.
        for (target, selected) in prepared.targets.iter().zip(&plan.seals.targets) {
            target.with_writer_io(&plan.seals.io.admission_hash, |root| {
                let local = plan.local(root)?;
                local.preflight(root)?;
                if local.completed(root)? {
                    local.require_complete(root)?;
                    plan.verify_target(root, &selected.identity.disk_id, stop)?;
                } else {
                    plan.inventory
                        .verify_present(root, store::Presence::Existing, stop)?;
                }
                plan.inventory.require_capacity(root, reserve)
            })?;
        }
        install_preparation_bytes(
            context.system,
            &context.system.path().join(INTENT),
            &plan.bytes,
        )?;
        for target in &prepared.targets {
            target.with_writer_io(&plan.seals.io.admission_hash, |root| {
                plan.local(root)?.select(root)
            })?;
        }
        for (target, selected) in prepared.targets.iter().zip(&plan.seals.targets) {
            preparation_checkpoint(stop)?;
            prepared.recheck_names()?;
            target.with_writer_io(&plan.seals.io.admission_hash, |root| {
                let local = plan.local(root)?;
                local.preflight(root)?;
                if !local.completed(root)? {
                    for (path, bytes) in plan.material.files() {
                        preparation_checkpoint(stop)?;
                        install_preparation_bytes(root, &root.path().join(path), bytes)?;
                    }
                }
                plan.verify_target(root, &selected.identity.disk_id, stop)?;
                local.complete(root)
            })?;
        }
    }
    // Drop the potentially large recipe before independent all-root replay.
    let original_bytes = plan.bytes.clone();
    drop(plan);
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if after.bytes != original_bytes {
        return Err(invalid(
            "Backup target locator inputs changed before common completion",
        ));
    }
    after.require_record(context.system, INTENT)?;
    after.verify_targets(prepared, stop)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        target.with_writer_io(&after.seals.io.admission_hash, |root| {
            install_preparation_bytes(root, &root.path().join(COMPLETE), &after.bytes)
        })?;
    }
    context.sources.for_each_disk(
        stop,
        |root, _| install_preparation_bytes(root, &root.path().join(COMPLETE), &after.bytes),
        &prepared.task.selection().first().pool_id,
    )?;
    install_preparation_bytes(
        context.system,
        &context.system.path().join(COMPLETE),
        &after.bytes,
    )?;
    target_writers::visit(context, prepared, stop, |root| {
        after.require_record(root, COMPLETE)
    })?;
    prepared.recheck_names()?;
    context.sources.recheck(context.system)?;
    Ok(after.report())
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetLocatorReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_record(context.system, INTENT)?;
    plan.verify_targets(prepared, stop)?;
    target_writers::visit(context, prepared, stop, |root| {
        plan.require_record(root, COMPLETE)
    })?;
    Ok(plan.report())
}
