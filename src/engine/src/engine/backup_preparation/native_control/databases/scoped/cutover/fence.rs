//! Typed live verification capability retained until the native source fence
//! append finishes. Persisted evidence is descriptive; it never grants writes.
use super::*;
use crate::engine::backup_preparation::native_control::runtime::Target;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct BackupSourceFenceEvidence {
    task: CutoverTask,
    report: BackupCutoverPreparationReport,
}
impl std::fmt::Debug for BackupSourceFenceEvidence {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("BackupSourceFenceEvidence")
            .field("operation_id", &self.task.request.operation_id)
            .field("task_hash", &self.report.task_hash)
            .field("target_count", &self.report.targets.len())
            .finish()
    }
}
impl BackupSourceFenceEvidence {
    pub(in crate::engine) fn validate(
        &self,
        config: &crate::ValidatedConfig,
        operation: OperationId,
        principal: &str,
        sequence: u64,
        snapshot: &str,
    ) -> Result<()> {
        self.task.validate()?;
        let job = self.task.selection().first();
        if config.generation().is_some()
            || self.task.request.operation_id != operation
            || self.task.request.principal != principal
            || object_hash("backup-preparation-config", config)? != job.configuration_hash
            || sequence != job.request.expected_sequence
            || snapshot != job.snapshot
            || self.report.format != FORMAT
            || self.report.task_hash != object_hash(FORMAT, &self.task)?
            || self.report.scoped.format != super::super::FORMAT
            || self.report.scoped.task_hash != object_hash(super::super::FORMAT, &self.task.scoped)?
            || self.report.scoped.objects.format != super::super::super::FORMAT
            || self.report.scoped.objects.task_hash
                != object_hash(super::super::super::FORMAT, &self.task.scoped.objects)?
            || self.report.scoped.objects.native.format != super::super::super::super::FORMAT
            || self.report.scoped.objects.native.targets.len() != self.task.selection().jobs().len()
            || self.report.original_source != self.task.topology.source
            || self.report.replaced_terminal != self.task.topology.replaced
            || self.report.targets != self.task.topology.nodes
            || self.report.whole_file_units_per_target
                != self.report.scoped.objects.files_per_target
            || self.report.directory_repositories_per_target
                != self.report.scoped.objects.directories_per_target
        {
            return Err(invalid(
                "backup source fence evidence differs from its exact prepared source and complete target set",
            ));
        }
        self.task
            .scoped
            .objects
            .check_native(&self.report.scoped.objects.native)?;
        for (observed, job) in self
            .report
            .scoped
            .objects
            .native
            .targets
            .iter()
            .zip(self.task.selection().jobs())
        {
            if observed.material.job_hash != object_hash("backup-preparation-job", job)?
                || observed.material.sequence != sequence
                || observed.material.snapshot_hash != snapshot
                || observed.control.sequence != sequence
                || observed.control.snapshot_hash != snapshot
                || observed.control.state_hash != self.task.scoped.objects.state_hash
                || observed.control.archive_hash != self.task.scoped.objects.native.archive_hash
                || observed.control.original_members != config.metadata_members().len()
            {
                return Err(invalid(
                    "backup source fence target evidence differs from the original frozen task",
                ));
            }
        }
        // This entire envelope is embedded in every original transition object,
        // rather than depending on a SYSTEM-local job for cold source fencing.
        if canonical_json(self)?.len() > 16 * 1024 * 1024 {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "complete backup source fence evidence exceeds 16 MiB",
            ));
        }
        Ok(())
    }
    pub(in crate::engine) fn state_hash(&self) -> &str {
        &self.task.scoped.objects.state_hash
    }
}

pub(in crate::engine) struct PreparedBackupSourceFence {
    pub(in crate::engine) evidence: BackupSourceFenceEvidence,
    targets: Vec<Target>,
}
impl PreparedBackupSourceFence {
    pub(in crate::engine) fn recheck_targets(&self) -> Result<()> {
        for (target, job) in self
            .targets
            .iter()
            .zip(self.evidence.task.selection().jobs())
        {
            named_target(&target.root, job)?;
        }
        Ok(())
    }
}

impl Engine {
    pub(in crate::engine) fn prepare_backup_source_fence(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<PreparedBackupSourceFence> {
        Self::require_actor(principal)?;
        self.require_write()?;
        self.require_unfenced_membership()?;
        let task = CutoverTask::read(&DiskRoot::open(&self.root)?)?;
        if task.request.operation_id != operation || task.request.principal != principal {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "source fence belongs to another original backup task or actor",
            ));
        }
        if self
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
        {
            return Err(Error::new(
                ErrorCode::WorkspaceBusy,
                "source fencing cannot overlap a running material worker",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        let targets = super::super::super::super::runtime::prepare(self, task.selection())?;
        let report = verify_at(
            &task,
            &targets
                .iter()
                .map(|target| &target.root)
                .collect::<Vec<_>>(),
            &AtomicBool::new(false),
        )?;
        let evidence = BackupSourceFenceEvidence { task, report };
        let snapshot = self
            .snapshot
            .as_deref()
            .ok_or_else(|| invalid("backup source fence needs a published snapshot"))?;
        evidence.validate(
            &self.config,
            operation,
            principal,
            self.state.sequence,
            snapshot,
        )?;
        if evidence.state_hash() != object_hash("backup-prepared-state", &*self.state)? {
            return Err(invalid(
                "source State differs from the independently prepared complete targets",
            ));
        }
        let prepared = PreparedBackupSourceFence { evidence, targets };
        prepared.recheck_targets()?;
        Ok(prepared)
    }
}
