//! Install the exact native target DiskWriter identity at its ordinary address.
//! A seal is not a selector: the source Engine stays fenced, and no generation
//! marker, active Current, ordinary pool writer or source release is installed.
use super::*;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

mod store;
pub(super) use store::require_staging;

const INTENT: &str = "backup-preparation/target-seals.json";
const COMPLETE: &str = "backup-target-seals-installed.json";
const MAX_RECORD: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct BackupTargetSealReport {
    pub identity: DiskRegistrationIdentity,
    pub seal_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub seal_device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub seal_inode: u64,
}

/// Actual no-clobber disk.json files under retained task-local I/O. This is not
/// a completed pool switch, normal startup admission or permission to unfence.
#[derive(Clone, Debug)]
pub struct BackupTargetSealsReport {
    pub io: BackupWriterIoReport,
    pub installation_hash: String,
    pub targets: Vec<BackupTargetSealReport>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    admission_hash: &'a str,
    native_installation_hash: &'a str,
    seals: &'a [DiskSeal],
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    plan_hash: &'a str,
    targets: &'a [BackupTargetSealReport],
}
struct Plan {
    io: BackupWriterIoReport,
    seals: Vec<DiskSeal>,
    bytes: Vec<u8>,
    hash: String,
}

fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_RECORD {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "Backup target seal descriptor exceeds 64 KiB",
        ));
    }
    Ok(bytes)
}

impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        // This includes every actual original decision/Publication, independent
        // body/database/native image, writer inode and real target I/O gate.
        let io = writer_io::observe(context, prepared, decision, expected, stop)?;
        let seals = prepared
            .targets
            .iter()
            .map(|target| target.writer_seal(&io.admission_hash))
            .collect::<Result<Vec<_>>>()?;
        let bytes = bounded(&Manifest {
            format: "zxs-backup-target-seal-plan",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: decision.hash().as_str(),
            publication_hash: expected.as_str(),
            admission_hash: &io.admission_hash,
            native_installation_hash: &io.writers.native.installation_hash,
            seals: &seals,
        })?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        Ok(Self {
            io,
            seals,
            bytes,
            hash,
        })
    }

    fn local(&self, index: usize) -> Result<store::PreparedSeal> {
        store::PreparedSeal::new(
            &self.hash,
            &self.io.writers.native.installation_hash,
            &self.io.admission_hash,
            self.seals[index].clone(),
        )
    }

    fn require_intent(&self, system: &DiskRoot) -> Result<()> {
        if system.read_bytes(&system.path().join(INTENT), MAX_RECORD)? != self.bytes {
            return Err(invalid(
                "target seal plan differs from the exact native writer admission",
            ));
        }
        Ok(())
    }

    fn observe_targets(
        &self,
        prepared: &PreparedBackupRegistry,
    ) -> Result<Vec<BackupTargetSealReport>> {
        let mut reports = Vec::with_capacity(prepared.targets.len());
        for (index, target) in prepared.targets.iter().enumerate() {
            let local = self.local(index)?;
            let binding =
                target.with_writer_io(&self.io.admission_hash, |root| local.observe(root))?;
            reports.push(BackupTargetSealReport {
                identity: self.io.targets[index].identity.clone(),
                seal_hash: local.seal_hash(),
                seal_device: binding.device,
                seal_inode: binding.inode,
            });
        }
        prepared.recheck_names()?;
        Ok(reports)
    }
}

fn completion(plan: &Plan, targets: &[BackupTargetSealReport]) -> Result<Vec<u8>> {
    bounded(&Completion {
        format: "zxs-backup-target-seals",
        plan_hash: &plan.hash,
        targets,
    })
}

fn require_completion(root: &DiskRoot, expected: &[u8]) -> Result<()> {
    if root.read_bytes(&root.path().join(COMPLETE), MAX_RECORD)? != expected {
        return Err(invalid(
            "target seal completion changes its original plan or file identities",
        ));
    }
    Ok(())
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetSealsReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    let mut complete = false;
    target_writers::visit(context, prepared, stop, |root| {
        complete |= preparation_path_exists(root, &root.path().join(COMPLETE))?;
        Ok(())
    })?;
    if complete {
        // ANY common completion follows every local file binding. A lost seal,
        // intent or binding is no longer an interrupted creation to finish.
        plan.require_intent(context.system)?;
        let targets = plan.observe_targets(prepared)?;
        let bytes = completion(&plan, &targets)?;
        target_writers::visit(context, prepared, stop, |root| {
            if preparation_path_exists(root, &root.path().join(COMPLETE))? {
                require_completion(root, &bytes)?;
            }
            Ok(())
        })?;
    } else {
        if preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
            plan.require_intent(context.system)?;
        }
        let required = context
            .sources
            .metadata_reserve_bytes()
            .checked_add(1024 * 1024)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "Backup seal reserve overflow"))?;
        target_writers::visit(context, prepared, stop, |root| {
            if root.available()? < required {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "Backup target seals need the original metadata reserve plus 1 MiB per root",
                ));
            }
            Ok(())
        })?;
        for (index, target) in prepared.targets.iter().enumerate() {
            let local = plan.local(index)?;
            target.with_writer_io(&plan.io.admission_hash, |root| local.preflight(root))?;
        }
        install_preparation_bytes(
            context.system,
            &context.system.path().join(INTENT),
            &plan.bytes,
        )?;
        for (index, target) in prepared.targets.iter().enumerate() {
            preparation_checkpoint(stop)?;
            prepared.recheck_names()?;
            let local = plan.local(index)?;
            target.with_writer_io(&plan.io.admission_hash, |root| local.install(root))?;
        }
        plan.observe_targets(prepared)?;
    }
    // Actual seals remain behind the no-selector boundary while the complete
    // all-root proof is read again. No source seal or generation is promoted.
    let before = plan.bytes;
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if after.bytes != before {
        return Err(invalid(
            "Backup target seal inputs changed during installation",
        ));
    }
    after.require_intent(context.system)?;
    let targets = after.observe_targets(prepared)?;
    let bytes = completion(&after, &targets)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        target.with_writer_io(&after.io.admission_hash, |root| {
            install_preparation_bytes(root, &root.path().join(COMPLETE), &bytes)
        })?;
    }
    context.sources.for_each_disk(
        stop,
        |root, _| install_preparation_bytes(root, &root.path().join(COMPLETE), &bytes),
        &prepared.task.selection().first().pool_id,
    )?;
    install_preparation_bytes(
        context.system,
        &context.system.path().join(COMPLETE),
        &bytes,
    )?;
    target_writers::visit(context, prepared, stop, |root| {
        require_completion(root, &bytes)
    })?;
    after.observe_targets(prepared)?;
    Ok(BackupTargetSealsReport {
        io: after.io,
        installation_hash: blake3::hash(&bytes).to_hex().to_string(),
        targets,
    })
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetSealsReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_intent(context.system)?;
    let targets = plan.observe_targets(prepared)?;
    let bytes = completion(&plan, &targets)?;
    target_writers::visit(context, prepared, stop, |root| {
        require_completion(root, &bytes)
    })?;
    Ok(BackupTargetSealsReport {
        io: plan.io,
        installation_hash: blake3::hash(&bytes).to_hex().to_string(),
        targets,
    })
}
