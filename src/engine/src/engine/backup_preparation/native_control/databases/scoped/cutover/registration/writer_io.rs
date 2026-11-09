//! Target-only ordinary DiskWriter admission for the exact frozen task. The
//! original Engine stays in its source configuration with every write fence.
use super::*;
use crate::engine::membership_transition::pending::activation::BackupWriterIoConfig;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

const INTENT: &str = "backup-preparation/target-writer-io.json";
const COMPLETE: &str = "backup-target-writer-io-admitted.json";
const MAX_RECORD: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct BackupTargetWriterIoReport {
    pub identity: DiskRegistrationIdentity,
    pub admission_epoch: String,
}

/// Fresh task-local DiskWriter I/O, not ordinary Engine readiness. The source
/// remains fenced and no selector, serving seal or user publication is created.
#[derive(Clone, Debug)]
pub struct BackupWriterIoReport {
    pub writers: BackupTargetWritersReport,
    pub admission_hash: String,
    pub config_hash: String,
    pub targets: Vec<BackupTargetWriterIoReport>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    fencing_hash: &'a str,
    publication_hash: &'a str,
    authority_hash: &'a str,
    candidate_hash: &'a str,
    config_hash: &'a str,
    registry: &'a DiskRegistryCommitment,
    targets: &'a [BackupTargetWriterIoReport],
}
struct Plan {
    admitted: BackupWriterIoConfig,
    bytes: Vec<u8>,
    report: BackupWriterIoReport,
}
impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        // A progress record, a target lock and the prospective State result
        // alone cannot grant this capability. Re-observe every real proof first.
        let writers = target_writers::observe(context, prepared, decision, expected, stop)?;
        let (_, candidate) = first_candidate::read_verified(prepared, decision, stop)?;
        let publication = context
            .choices
            .read_first_publication(prepared, &candidate, stop)?;
        if publication.report().publication_hash != expected.as_str()
            || candidate.hash() != writers.native.candidate_hash
            || candidate.authority_hash() != writers.native.authority_hash
        {
            return Err(invalid(
                "Backup writer I/O lost its exact observed native publication",
            ));
        }
        let admitted = candidate.writer_configuration(
            &publication,
            generation_control::retained_archive(prepared, 0)?,
            stop,
        )?;
        let config = admitted.config();
        let config_hash = object_hash("backup-effective-config", config)?;
        let targets = prepared
            .projection
            .targets
            .iter()
            .map(|selected| {
                Ok(BackupTargetWriterIoReport {
                    identity: selected.node.identity.clone(),
                    admission_epoch: config
                        .io_admission_epoch(&selected.node.identity.disk_id)?
                        .expect("invariant: selected Backup target has native I/O epoch"),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let bytes = canonical_json(&Manifest {
            format: "zxs-backup-target-writer-io",
            source_fence_hash: prepared.source_fence_hash(),
            fencing_hash: &writers.fencing_hash,
            publication_hash: expected.as_str(),
            authority_hash: &writers.native.authority_hash,
            candidate_hash: &writers.native.candidate_hash,
            config_hash: &config_hash,
            registry: &writers.native.registry,
            targets: &targets,
        })?;
        if bytes.len() > MAX_RECORD {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup target writer I/O descriptor exceeds 64 KiB",
            ));
        }
        let report = BackupWriterIoReport {
            admission_hash: blake3::hash(&bytes).to_hex().to_string(),
            writers,
            config_hash,
            targets,
        };
        Ok(Self {
            admitted,
            bytes,
            report,
        })
    }

    fn require_record(&self, root: &DiskRoot, name: &str) -> Result<()> {
        if root.read_bytes(&root.path().join(name), MAX_RECORD)? != self.bytes {
            return Err(invalid(
                "Backup writer I/O record changes the original complete admission",
            ));
        }
        Ok(())
    }

    fn bind_targets(&self, prepared: &PreparedBackupRegistry, stop: &AtomicBool) -> Result<()> {
        for ((target, selected), writer) in prepared
            .targets
            .iter()
            .zip(&self.report.targets)
            .zip(&self.report.writers.targets)
        {
            preparation_checkpoint(stop)?;
            let epoch = target.bind_writer_io(
                &self.admitted,
                &selected.identity.disk_id,
                &PhysicalBinding {
                    device: writer.writer_device,
                    inode: writer.writer_inode,
                },
                &self.report.admission_hash,
            )?;
            if epoch != selected.admission_epoch || selected.identity != writer.identity {
                return Err(invalid(
                    "Backup writer I/O differs from its exact selected target binding",
                ));
            }
        }
        prepared.recheck_names()
    }

    fn require_targets(&self, prepared: &PreparedBackupRegistry) -> Result<()> {
        for target in &prepared.targets {
            target.with_writer_io(&self.report.admission_hash, |root| {
                self.require_record(root, COMPLETE)
            })?;
        }
        prepared.recheck_names()
    }
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupWriterIoReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    let mut complete = false;
    target_writers::visit(context, prepared, stop, |root| {
        if preparation_path_exists(root, &root.path().join(COMPLETE))? {
            plan.require_record(root, COMPLETE)?;
            complete = true;
        }
        Ok(())
    })?;
    if complete || preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
        plan.require_record(context.system, INTENT)?;
    }
    let required = context
        .sources
        .metadata_reserve_bytes()
        .checked_add(1024 * 1024)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "Backup writer I/O reserve overflow",
            )
        })?;
    target_writers::visit(context, prepared, stop, |root| {
        if root.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "Backup writer I/O needs the original metadata reserve plus 1 MiB per root",
            ));
        }
        Ok(())
    })?;
    install_preparation_bytes(
        context.system,
        &context.system.path().join(INTENT),
        &plan.bytes,
    )?;
    plan.bind_targets(prepared, stop)?;

    // Re-observe ordinary native objects, real bodies and databases through the
    // permanently bound target roots. An older alias cannot revert to bootstrap
    // I/O if a gate closes. No source binding or authority is advanced here.
    let fresh = target_writers::observe(context, prepared, decision, expected, stop)?;
    if fresh.fencing_hash != plan.report.writers.fencing_hash {
        return Err(invalid(
            "Backup writer I/O proof changed during target admission",
        ));
    }
    let after = plan;
    after.bind_targets(prepared, stop)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        target.with_writer_io(&after.report.admission_hash, |root| {
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
    after.require_targets(prepared)?;
    target_writers::visit(context, prepared, stop, |root| {
        after.require_record(root, COMPLETE)
    })?;
    Ok(after.report)
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupWriterIoReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_record(context.system, INTENT)?;
    plan.require_targets(prepared)?;
    target_writers::visit(context, prepared, stop, |root| {
        plan.require_record(root, COMPLETE)
    })?;
    Ok(plan.report)
}
