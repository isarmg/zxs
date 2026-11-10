//! Backup's one full-root bootstrap transaction. Ordinary startup requires a
//! completed exact expansion. Only explicit same-operation recovery may inspect
//! a mixed old/new selector set, while retaining every existing writer lock.
use super::*;
use crate::DiskConfig;
use crate::disk::{DiskHandle, DiskRoot};
use crate::engine::backup_preparation::ServingBarrier;
use crate::engine::disk_lifecycle::bootstrap::{
    backup_root_extension::{observe_fixed_roots, open_fixed_roots},
    runtime::{management_error, open_store},
    store::{BootstrapStore, PreparedRootExtension},
};
use crate::engine::membership_transition::{
    BackupFirstPublicationHash, PreparedBackupFirstCandidate, PreparedBackupFirstPublication,
};
use crate::history_package::{AdmittedState, preparation_checkpoint as checkpoint};
use std::{fs::File, path::Path};
mod native;
mod resume;
mod store;
pub(in crate::engine) use resume::prepare_resume;
use store::{Plan, PlanStore};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::RecoveryRequired, message)
}

/// Native evidence only. It is never serialized, constructed from a completion
/// digest, or usable to admit a different pool/operation/root set.
pub(in crate::engine) struct BackupStartupAdmission {
    plan: Plan,
    native: native::Native,
    roots: Vec<(DiskConfig, DiskRoot)>,
    config: ValidatedConfig,
    completed: bool,
}
impl BackupStartupAdmission {
    pub(in crate::engine) fn first(&self) -> &PreparedBackupFirstCandidate {
        &self.native.first
    }
    pub(in crate::engine) fn publication(&self) -> &PreparedBackupFirstPublication {
        &self.native.publication
    }
    pub(in crate::engine) fn registry(&self) -> &crate::VerifiedDiskRegistry {
        &self.native.registry
    }
    pub(in crate::engine) fn published(&self) -> &AdmittedState {
        &self.native.published
    }
    pub(in crate::engine) fn source(&self) -> &AdmittedState {
        &self.native.source
    }
    pub(in crate::engine) fn config(&self) -> &ValidatedConfig {
        &self.config
    }
    pub(in crate::engine) fn held_roots(&self) -> &[(DiskConfig, DiskRoot)] {
        &self.roots
    }
    pub(in crate::engine) fn require_config(&self, config: &ValidatedConfig) -> Result<()> {
        if !self.completed {
            return Err(invalid(
                "unfinished Backup probe does not grant ordinary startup resources",
            ));
        }
        self.plan.require_config(config)?;
        if self.roots.len() != config.disks.len() {
            return Err(invalid("Backup startup lost a required root"));
        }
        Ok(())
    }
    /// Complete original publication replay, not a latest-State shortcut. Each
    /// supplied State is independently admitted before the Current consumer.
    pub(in crate::engine) fn replay_source(
        &self,
        stop: &AtomicBool,
        mut visit: impl FnMut(&Candidate, &AdmittedState) -> Result<()>,
    ) -> Result<()> {
        let root = &self
            .roots
            .first()
            .ok_or_else(|| invalid("Backup replay has no fixed archive root"))?
            .1;
        let mut count = 0usize;
        crate::engine::membership_transition::archive::persistent::namespace_replay::replay_installed_publications(
            native::archive(root, &self.plan.authority)?, &self.plan.authority.archive_hash()?,
            &self.plan.authority.archive_transition()?, |item| {
                checkpoint(stop)?;
                count += 1;
                if count > 2048 { return Err(Error::new(ErrorCode::ResourceLimit, "Backup source replay exceeds 2048 Publications")); }
                if item.activation().is_some() || item.candidate().generation().is_some() {
                    return Err(invalid("Backup source replay is not the selected fixed Genesis archive"));
                }
                let state = AdmittedState::admit_replayed(item.state().as_ref().clone())?;
                visit(item.candidate(), &state)
            })?;
        checkpoint(stop)
    }
}

fn authenticate(
    plan: Plan,
    roots: Vec<(DiskConfig, DiskRoot)>,
    projected: &ValidatedConfig,
    completed: bool,
    stop: &AtomicBool,
) -> Result<BackupStartupAdmission> {
    let effective = plan.authority.effective_config()?;
    if projected.pool_id != effective.pool_id
        || !plan.authority.matches_metadata_projection(projected)?
        || roots.len() != effective.disks.len()
        || roots.is_empty()
    {
        return Err(invalid(
            "Backup discovery changes the native fixed original membership",
        ));
    }
    let mut selected: Option<native::Native> = None;
    for (disk, root) in &roots {
        let actual = native::read(&plan, root, disk, completed, stop)?;
        if let Some(previous) = &selected {
            if previous.first.hash() != actual.first.hash()
                || previous.registry.commitment() != actual.registry.commitment()
                || previous.publication.publication_bytes()
                    != actual.publication.publication_bytes()
                || previous.publication.seal_bytes() != actual.publication.seal_bytes()
            {
                return Err(invalid(
                    "Backup roots disagree on original native Publication or complete registry",
                ));
            }
        } else {
            selected = Some(actual);
        }
    }
    let native = selected.expect("invariant: nonempty fixed root set verified");
    let config = native
        .writer
        .config()
        .clone()
        .with_workspace_limits(Some(projected.effective_workspace_limits()))?;
    let mut admission = BackupStartupAdmission {
        plan,
        native,
        roots,
        config,
        completed,
    };
    if completed {
        admission.config = admission
            .config
            .clone()
            .with_backup_startup_resources(&admission)?;
    }
    Ok(admission)
}

/// Runs with the ordinary discovery bootstrap guard still held, before any
/// Config-root fallback or Add verifier. An unfinished selected Backup closes
/// ordinary open even if every metadata voter still carries the old marker.
pub(in crate::engine::membership_transition::pending::activation) fn discover_completed(
    root: &Path,
    projected: &ValidatedConfig,
    bootstrap: &BootstrapStore,
) -> Result<Option<BackupStartupAdmission>> {
    let Some((saved, plan)) = PlanStore::open(root)? else {
        return Ok(None);
    };
    if !saved.completed()? {
        return Err(invalid(
            "unfinished Backup switch requires explicit same-operation resume",
        ));
    }
    let locator = bootstrap
        .root_extension_locator(&plan.extension)
        .map_err(management_error)?;
    // A completion without its installed head is not permission to repair it.
    bootstrap
        .reload(&projected.pool_id)
        .map_err(management_error)?;
    bootstrap
        .require_published_root_extension(&plan.extension)
        .map_err(management_error)?;
    let roots = open_fixed_roots(&locator, &plan.authority.effective_config()?)?;
    let admission = authenticate(plan, roots, projected, true, &AtomicBool::new(false))?;
    saved.verify()?;
    Ok(Some(admission))
}

/// Recheck the resources already admitted under the still-held startup locator
/// and real Engine writer locks. Do not recursively acquire that locator lock.
pub(in crate::engine) fn verify_completed(engine: &Engine) -> Result<BackupStartupAdmission> {
    let (saved, plan) = PlanStore::open(&engine.root)?
        .ok_or_else(|| invalid("Backup startup lost its selected bootstrap plan"))?;
    if !saved.completed()? {
        return Err(invalid("Backup startup plan is not completed"));
    }
    saved.verify_writer(engine)?;
    plan.require_config(&engine.config)?;
    let mut roots = Vec::with_capacity(engine.config.disks.len());
    for disk in &engine.config.disks {
        let handle = engine.disk_resources.get(disk)?;
        engine._locks.verify_disk(disk, &handle.root)?;
        roots.push((disk.clone(), handle.root.retained(disk.root.clone())?));
    }
    let admission = authenticate(plan, roots, &engine.config, true, &AtomicBool::new(false))?;
    saved.verify_writer(engine)?;
    Ok(admission)
}

pub(in crate::engine) struct PreparedBackupBootstrap<'a> {
    engine: &'a Engine,
    first: &'a PreparedBackupFirstCandidate,
    publication: &'a PreparedBackupFirstPublication,
    bootstrap: BootstrapStore,
    saved: PlanStore,
    plan: Plan,
    roots: Vec<(DiskConfig, DiskRoot)>,
    config: ValidatedConfig,
    admission: BackupStartupAdmission,
}
impl<'a> PreparedBackupBootstrap<'a> {
    pub(in crate::engine) fn select(
        engine: &'a Engine,
        admitted: &BackupWriterIoConfig,
        first: &'a PreparedBackupFirstCandidate,
        publication: &'a PreparedBackupFirstPublication,
        targets: &[Arc<DiskHandle>],
    ) -> Result<Self> {
        let config = admitted.config();
        let generation = config
            .generation()
            .ok_or_else(|| invalid("Backup expansion lacks its native continuation"))?;
        if !generation.portable.is_backup()
            || engine.config.generation().is_some()
            || !authority_config_matches(&engine.config, first.original_config())
            || engine
                .membership_fence
                .as_ref()
                .is_none_or(|fence| fence.hash != first.candidate().request_digest)
        {
            return Err(invalid(
                "Backup expansion requires the exact fenced original Engine",
            ));
        }
        publication.require_namespace_candidate(first)?;
        let target_ids = generation
            .portable
            .backup_core()
            .ok_or_else(|| invalid("Backup expansion has another authority"))?
            .writer_targets()?;
        if targets.len() != target_ids.len() {
            return Err(invalid("Backup expansion lacks a selected target"));
        }
        let mut roots = Vec::with_capacity(config.disks.len());
        let mut seen = BTreeSet::new();
        for disk in &config.disks {
            let handle = if target_ids.contains(&disk.disk_id) {
                targets
                    .iter()
                    .find(|target| target.seal.disk_id == disk.disk_id)
                    .ok_or_else(|| invalid("Backup target handle is absent"))?
            } else {
                let old = engine
                    .config
                    .disks
                    .iter()
                    .find(|old| old.disk_id == disk.disk_id)
                    .ok_or_else(|| invalid("Backup expansion invents another original disk"))?;
                let handle = engine.disk_resources.get(old)?;
                engine._locks.verify_disk(old, &handle.root)?;
                handle
            };
            if !seen.insert(handle.seal.disk_id.clone()) {
                return Err(invalid("Backup expansion repeats a physical member"));
            }
            roots.push((disk.clone(), handle.root.retained(disk.root.clone())?));
        }
        let bootstrap = open_store(&engine.root)?;
        let observations = observe_fixed_roots(config, &roots)?;
        let (saved, plan) = match PlanStore::open(&engine.root)? {
            Some((saved, plan)) => {
                if plan.operation()? != first.result().operation_id
                    || plan.candidate != first.hash()
                    || plan.publication != publication.report().publication_hash
                {
                    return Err(invalid(
                        "Backup expansion is selected for another native operation",
                    ));
                }
                plan.require_config(config)?;
                saved.verify_writer(engine)?;
                (saved, plan)
            }
            None => {
                let previous = bootstrap
                    .reload(&config.pool_id)
                    .map_err(management_error)?;
                let extension = bootstrap
                    .prepare_root_extension(&previous, observations)
                    .map_err(management_error)?
                    .ok_or_else(|| {
                        invalid("Backup roots were expanded without this original protected plan")
                    })?;
                let plan = Plan::capture(admitted, first, publication, extension)?;
                let saved = PlanStore::select(engine, &plan)?;
                (saved, plan)
            }
        };
        let locator = bootstrap
            .root_extension_locator(&plan.extension)
            .map_err(management_error)?;
        // Independently match every plan identity before the caller may start
        // selectors. No writes, repairs or dropped original descriptors here.
        drop(open_fixed_roots(&locator, config)?);
        let completed = saved.completed()?;
        if completed {
            bootstrap
                .require_published_root_extension(&plan.extension)
                .map_err(management_error)?;
        }
        let admitted_roots = roots
            .iter()
            .map(|(disk, root)| Ok((disk.clone(), root.retained(disk.root.clone())?)))
            .collect::<Result<Vec<_>>>()?;
        // This is still the original live first handoff. Even after bootstrap
        // completed, no ordinary successor slot may be silently adopted here.
        let admission = authenticate(
            plan.clone(),
            admitted_roots,
            &engine.config,
            false,
            &AtomicBool::new(false),
        )?;
        if admission.first().hash() != first.hash()
            || admission.publication().publication_bytes() != publication.publication_bytes()
            || admission.publication().seal_bytes() != publication.seal_bytes()
            || admission.registry().commitment() != &plan.registry
            || engine.snapshot != first.candidate().previous_snapshot
            || engine.state.sequence != admission.source().state().sequence
        {
            return Err(invalid(
                "Backup live retry changed its native first handoff or frozen Engine",
            ));
        }
        for (disk, root) in &roots {
            native::require_selector(&plan, root, disk, config, completed)?;
        }
        Ok(Self {
            engine,
            first,
            publication,
            bootstrap,
            saved,
            plan,
            roots,
            config: config.clone(),
            admission,
        })
    }
    pub(in crate::engine) fn admission(&self) -> &BackupStartupAdmission {
        &self.admission
    }
    pub(in crate::engine) fn publish(mut self, barrier: &ServingBarrier) -> Result<()> {
        barrier.require_native(self.first, self.publication, &self.plan.registry)?;
        barrier.require_config(&self.config)?;
        self.saved.verify_writer(self.engine)?;
        for (disk, root) in &self.roots {
            native::require_selector(&self.plan, root, disk, &self.config, true)?;
        }
        if self.saved.completed()? {
            // Completion cannot authorize repairing a missing/replaced head.
            self.bootstrap
                .require_published_root_extension(&self.plan.extension)
                .map_err(management_error)?;
            return self.saved.complete();
        }
        let observations = observe_fixed_roots(&self.config, &self.roots)?;
        self.bootstrap
            .resume_root_extension(&self.plan.extension, observations)
            .map_err(management_error)?;
        self.bootstrap
            .require_published_root_extension(&self.plan.extension)
            .map_err(management_error)?;
        self.saved.complete()
    }
}
