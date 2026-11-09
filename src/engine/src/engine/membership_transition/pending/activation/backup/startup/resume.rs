//! Explicit same-operation cold continuation. Missing lock files, selector
//! mismatches and replaced roots are loss, never instructions to initialize.
use super::*;
use crate::disk::{DiskResources, DiskWriter, FenceOwner, WriterLocks};
use platform_io::fs_safety::linux::{AdvisoryLock, FileIdentity, SingleLinkRequirement};
use std::path::PathBuf;

pub(in crate::engine) struct PreparedBackupResume {
    root: PathBuf,
    bootstrap: BootstrapStore,
    saved: PlanStore,
    admission: BackupStartupAdmission,
    system: File,
    selection: crate::authority_selection::SelectionGuard,
    writers: BTreeMap<String, Arc<FenceOwner>>,
}
impl PreparedBackupResume {
    pub(in crate::engine) fn admission(&self) -> &BackupStartupAdmission {
        &self.admission
    }
    pub(in crate::engine) fn held_roots(&self) -> &[(DiskConfig, DiskRoot)] {
        self.admission.held_roots()
    }
    pub(in crate::engine) fn require_owners(&self) -> Result<()> {
        self.saved.verify()?;
        verify_owners(
            &self.root,
            &self.system,
            &self.writers,
            &self.admission.roots,
        )
    }
    /// The coordinator has verified its immutable common selection, original
    /// DB completions and exact retained old/new seal-stage pairs on all roots.
    /// Only its private native barrier can publish the prepared root expansion.
    pub(in crate::engine) fn publish(
        mut self,
        barrier: &ServingBarrier,
    ) -> Result<ResumeResources> {
        self.require_owners()?;
        barrier.require_native(
            self.admission.first(),
            self.admission.publication(),
            self.admission.registry().commitment(),
        )?;
        barrier.require_config(self.admission.config())?;
        let roots = self
            .admission
            .roots
            .iter()
            .map(|(disk, root)| Ok((disk.clone(), root.retained(disk.root.clone())?)))
            .collect::<Result<Vec<_>>>()?;
        let completed = authenticate(
            self.admission.plan.clone(),
            roots,
            self.admission.config(),
            true,
            &AtomicBool::new(false),
        )?;
        let observations = observe_fixed_roots(completed.config(), completed.held_roots())?;
        self.bootstrap
            .resume_root_extension(&completed.plan.extension, observations)
            .map_err(management_error)?;
        self.bootstrap
            .require_published_root_extension(&completed.plan.extension)
            .map_err(management_error)?;
        self.require_owners()?;
        self.saved.complete()?;
        Ok(ResumeResources {
            root: self.root,
            bootstrap: self.bootstrap,
            admission: completed,
            system: self.system,
            selection: self.selection,
            writers: self.writers,
        })
    }
}

pub(in crate::engine) struct ResumeResources {
    root: PathBuf,
    bootstrap: BootstrapStore,
    admission: BackupStartupAdmission,
    system: File,
    selection: crate::authority_selection::SelectionGuard,
    writers: BTreeMap<String, Arc<FenceOwner>>,
}
impl ResumeResources {
    /// All disk.json files have just passed native NEW-seal verification. The
    /// same acquired OS descriptions become DiskWriter owners without unlocking
    /// or reopening the writer files. The caller still performs full replay,
    /// Current installation and SYSTEM rebuild before granting business writes.
    pub(in crate::engine) fn into_engine(self) -> Result<Engine> {
        verify_owners(
            &self.root,
            &self.system,
            &self.writers,
            &self.admission.roots,
        )?;
        self.admission.require_config(self.admission.config())?;
        let config = self.admission.config().clone();
        let dirs = self
            .admission
            .roots
            .iter()
            .map(|(_, root)| root.directory(root.path()))
            .collect::<Result<Vec<_>>>()?;
        let resources = Arc::new(DiskResources::recover_held(&config, dirs)?);
        let mut locks = WriterLocks::new(self.system, self.selection);
        for disk in config.io_disks() {
            let owner = self
                .writers
                .get(&disk.disk_id)
                .ok_or_else(|| invalid("Backup recovery lost a retained writer owner"))?;
            locks.insert(
                disk.disk_id.clone(),
                DiskWriter::adopt(&resources.get(disk)?.root, Arc::clone(owner))?,
            );
        }
        let engine = Engine::recovery_resources_with_locks(self.root, config, resources, locks)?;
        drop(self.bootstrap);
        Ok(engine)
    }
}

/// No new operation is selected here. The SYSTEM record restricts the names;
/// actual complete archives, all original journal prefixes and all selected
/// target ordinary payloads prove the SAME already-published Backup operation.
pub(in crate::engine) fn prepare_resume(
    root: &Path,
    projected: &ValidatedConfig,
    operation: OperationId,
) -> Result<Option<PreparedBackupResume>> {
    let bootstrap = open_store(root)?;
    super::super::super::exit::stop_plan::require_no_pending_stop(root, &bootstrap)?;
    let Some((saved, plan)) = PlanStore::open(root)? else {
        return Ok(None);
    };
    if plan.operation()? != operation {
        return Err(invalid(
            "Backup recovery cannot replay a different selected operation",
        ));
    }
    if saved.completed()? {
        return Ok(None);
    }
    let locator = bootstrap
        .root_extension_locator(&plan.extension)
        .map_err(management_error)?;
    let roots = open_fixed_roots(&locator, &plan.authority.effective_config()?)?;
    let selection = crate::authority_selection::SelectionGuard::legacy(root)?;
    let system_root = DiskRoot::open(root)?;
    let system = lock_existing(&system_root, "writer.lock")?;
    let mut writers = BTreeMap::new();
    for (disk, held) in &roots {
        let reservation = Arc::new(
            AdvisoryLock::directory(&held.directory(held.path())?)
                .map_err(crate::disk::fs_error)?,
        );
        let writer = lock_existing(held, ".writer.lock")?;
        writers.insert(
            disk.disk_id.clone(),
            FenceOwner::with_advisory_reservation(writer, reservation),
        );
    }
    verify_owners(root, &system, &writers, &roots)?;
    let admission = authenticate(plan, roots, projected, false, &AtomicBool::new(false))?;
    saved.verify()?;
    Ok(Some(PreparedBackupResume {
        root: root.to_owned(),
        bootstrap,
        saved,
        admission,
        system,
        selection,
        writers,
    }))
}

fn lock_existing(root: &DiskRoot, name: &str) -> Result<File> {
    let file = root.open_regular(&root.path().join(name))?;
    SingleLinkRequirement::check(&file).map_err(crate::disk::fs_error)?;
    if file.metadata()?.len() != 0 {
        return Err(invalid(
            "Backup existing writer file contains unrelated bytes",
        ));
    }
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
        |source| {
            Error::from(std::io::Error::from(source)).context(
                ErrorCode::WriterAlreadyActive,
                "another owner holds a required Backup writer",
            )
        },
    )?;
    Ok(file)
}
fn verify_owners(
    root: &Path,
    system: &File,
    writers: &BTreeMap<String, Arc<FenceOwner>>,
    roots: &[(DiskConfig, DiskRoot)],
) -> Result<()> {
    let system_root = DiskRoot::open(root)?;
    let named = system_root.open_regular(&system_root.path().join("writer.lock"))?;
    SingleLinkRequirement::check(&named).map_err(crate::disk::fs_error)?;
    if FileIdentity::from_fd(&named)? != FileIdentity::from_fd(system)?
        || named.metadata()?.len() != 0
    {
        return Err(invalid("Backup SYSTEM writer was replaced"));
    }
    for (disk, held) in roots {
        let named = held.open_regular(&held.path().join(".writer.lock"))?;
        SingleLinkRequirement::check(&named).map_err(crate::disk::fs_error)?;
        let owner = writers
            .get(&disk.disk_id)
            .ok_or_else(|| invalid("Backup root has no retained writer"))?;
        if owner.writer_identity()? != FileIdentity::from_fd(&named)?
            || named.metadata()?.len() != 0
        {
            return Err(invalid(
                "Backup original writer was replaced during recovery",
            ));
        }
        let now = DiskRoot::open(held.path())?;
        if FileIdentity::from_fd(&held.directory(held.path())?)?
            != FileIdentity::from_fd(&now.directory(now.path())?)?
        {
            return Err(invalid("Backup original root was rebound during recovery"));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../../../test/engine/unit/backup_bootstrap_writer_tests.rs"]
mod tests;
