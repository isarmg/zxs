//! Exact original-switch recovery. Bootstrap controls permitted probes; native
//! archives/publications control authority; selected old/new inodes control the
//! remaining filesystem exchanges. None can replace another boundary.
use super::*;
use crate::engine::membership_transition::pending::activation::backup::startup::{
    self, BackupStartupAdmission,
};

fn selected(system: &DiskRoot, admitted: &BackupStartupAdmission) -> Result<(Selection, Vec<u8>)> {
    let bytes = system.read_bytes(&system.path().join(INTENT), MAX_PLAN)?;
    let plan: Selection = storage_contract::decode_strict(&bytes)?;
    let first = admitted.first();
    if plan.format != "zxs-backup-serving-switch"
        || plan.operation != first.result().operation_id
        || plan.authority != first.authority_hash()
        || plan.source_fence != first.result().request_digest
        || plan.decision != first.decision_hash().as_str()
        || plan.candidate != first.hash()
        || plan.publication != admitted.publication().report().publication_hash
        || plan.config_hash != object_hash("backup-effective-config", admitted.config())?
        || plan.registry != *admitted.registry().commitment()
        || plan.system != PhysicalBinding::of(system)?
        || canonical_json(&plan)? != bytes
    {
        return Err(invalid(
            "Backup final switch is not its complete original native operation",
        ));
    }
    let named = system.open_read_regular(&system.path().join("writer.lock"))?;
    let writer = platform_io::fs_safety::linux::FileIdentity::from_metadata(&named.metadata()?);
    if plan.system_writer
        != (PhysicalBinding {
            device: writer.device,
            inode: writer.inode,
        })
    {
        return Err(invalid(
            "Backup final switch lost its original SYSTEM writer inode",
        ));
    }
    require_bytes(system, SELECTED, &bytes, MAX_PLAN)?;
    for (_, root) in admitted.held_roots() {
        require_bytes(root, SELECTED, &bytes, MAX_PLAN)?;
    }
    let targets = admitted
        .held_roots()
        .iter()
        .filter(|(disk, _)| {
            !first
                .original_config()
                .disks
                .iter()
                .any(|old| old.disk_id == disk.disk_id)
        })
        .collect::<Vec<_>>();
    if targets.len() != plan.target_writers.len() || targets.len() != first.targets().len() {
        return Err(invalid(
            "Backup final selection changes its original target writer set",
        ));
    }
    for (((disk, root), expected), native) in targets
        .into_iter()
        .zip(&plan.target_writers)
        .zip(first.targets())
    {
        let writer = root.open_read_regular(&root.path().join(".writer.lock"))?;
        let actual =
            platform_io::fs_safety::linux::FileIdentity::from_metadata(&writer.metadata()?);
        if expected.identity != native.identity
            || expected.identity
                != (DiskRegistrationIdentity {
                    pool_id: admitted.config().pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                })
            || expected.writer_device != actual.device
            || expected.writer_inode != actual.inode
        {
            return Err(invalid(
                "Backup final selection changed an actual original target writer",
            ));
        }
    }
    Ok((plan, bytes))
}

/// An unfinished final switch must still own each exact first ordinary image,
/// including empty directories, SQLite and locator/entity inodes. Do not use
/// this after serving: ordinary publications may legitimately change them.
fn require_initial_targets(
    system: &DiskRoot,
    admitted: &BackupStartupAdmission,
    selection: &Selection,
    stop: &AtomicBool,
) -> Result<()> {
    let ordinary_plan = system.read_bytes(
        &system
            .path()
            .join("backup-preparation/ordinary-serving.json"),
        64 * 1024,
    )?;
    if blake3::hash(&ordinary_plan).to_hex().as_str() != selection.ordinary_set {
        return Err(invalid(
            "Backup final switch lost its selected ordinary entity plan",
        ));
    }
    for (node, expected) in admitted
        .first()
        .targets()
        .iter()
        .zip(&selection.target_writers)
    {
        preparation_checkpoint(stop)?;
        let (_, root) = admitted
            .held_roots()
            .iter()
            .find(|(disk, _)| disk.disk_id == node.identity.disk_id)
            .expect("invariant: selected() checked every native target binding");
        let image = crate::container_storage::backup_ordinary::Image::capture(
            admitted.published(),
            &node.identity.slot_id,
            &node.source_prefix,
            &admitted.publication().report().publication_hash,
            stop,
        )?;
        if image.hash() != expected.ordinary_recipe {
            return Err(invalid(
                "Backup resume target recipe differs from its native first State",
            ));
        }
        let actual = image.verify(root, stop)?;
        if blake3::hash(&actual).to_hex().as_str() != expected.ordinary_entities {
            return Err(invalid(
                "unfinished Backup target changed its selected ordinary entity inodes",
            ));
        }
        require_bytes(
            root,
            "backup-ordinary-serving-installed.json",
            &actual,
            32 * 1024 * 1024,
        )?;
    }
    Ok(())
}

fn barrier(admitted: &BackupStartupAdmission, selection: &Selection) -> Result<ServingBarrier> {
    // SYSTEM is derived, but the original switch's image binding is still
    // deterministic from this native first State, not a copied marker hash.
    let system =
        crate::engine::index::preparation::encode(admitted.published(), &AtomicBool::new(false))?;
    if blake3::hash(&system).to_hex().as_str() != selection.system_database_hash {
        return Err(invalid(
            "Backup switch SYSTEM recipe differs from its actual first State",
        ));
    }
    let first = admitted.first();
    Ok(ServingBarrier {
        operation: first.result().operation_id,
        authority: first.authority_hash().into(),
        candidate: first.hash().into(),
        publication: admitted.publication().report().publication_hash.clone(),
        registry: admitted.registry().commitment().clone(),
        config_hash: selection.config_hash.clone(),
        first_sequence: first.sequence(),
        first_snapshot: first.snapshot_hash().into(),
        system_database_hash: selection.system_database_hash.clone(),
        replayed_current: true,
    })
}

fn completion(
    system: &DiskRoot,
    admitted: &BackupStartupAdmission,
    selection: &[u8],
) -> Result<Completion> {
    let bytes = system.read_bytes(&system.path().join(COMPLETE), MAX_PLAN)?;
    let complete: Completion = storage_contract::decode_strict(&bytes)?;
    if complete.format != "zxs-backup-serving-completed"
        || complete.selection_hash != blake3::hash(selection).to_hex().as_str()
        || complete.native_files
            != roots::completed_files(admitted.held_roots(), admitted.config())?
        || complete.original_objects.len() != admitted.first().original_config().disks.len()
        || canonical_json(&complete)? != bytes
    {
        return Err(invalid(
            "completed Backup switch lost its exact native selected file set",
        ));
    }
    for (_, root) in admitted.held_roots() {
        require_bytes(root, COMPLETE, &bytes, MAX_PLAN)?;
    }
    Ok(complete)
}

impl Engine {
    pub(in crate::engine) fn prepare_backup_serving_resources(
        &mut self,
    ) -> Result<Vec<crate::engine::RecoverySealFailure>> {
        // The outer startup still owns bootstrap; this verifier reuses the
        // Engine's original writer owners instead of attempting another flock.
        let admitted = startup::verify_completed(self)?;
        let system = DiskRoot::open(&self.root)?;
        let (selection, bytes) = selected(&system, &admitted)?;
        roots::recover_originals(
            admitted.held_roots(),
            admitted.first(),
            admitted.config(),
            &selection.original_roots,
            &bytes,
            true,
        )?;
        completion(&system, &admitted, &bytes)?;
        let barrier = barrier(&admitted, &selection)?;
        let mut replay = BackupNamespaceReplay::new(admitted.first())?;
        admitted.replay_source(
            &AtomicBool::new(false),
            |candidate: &crate::journal::Candidate, state: &AdmittedState| {
                replay.observe(candidate, state)
            },
        )?;
        let current = Arc::new(replay.finish_serving(
            admitted.publication(),
            admitted.first(),
            admitted.registry(),
            admitted.published(),
            &barrier,
        )?);
        let target_count =
            admitted.config().disks.len() - admitted.first().original_config().disks.len();
        self.backup_serving = Some(
            crate::engine::backup_preparation::ServingAdmission::capture(
                barrier,
                current,
                admitted.first(),
                target_count,
            )?,
        );
        let generation = self
            .config
            .generation()
            .expect("invariant: native Backup startup");
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.next_slot = generation.first_sequence()?;
        Ok(Vec::new())
    }

    /// Explicit management recovery of a previously selected final activation.
    /// No old/new target decision, initial copy or source fence is invented.
    /// Returns the same owned Engine only after complete ordinary replay and
    /// Current/SYSTEM admission; the host still owns authentication/readiness.
    pub fn resume_backup_transaction_at_with_workspace_limits(
        root: &Path,
        operation: OperationId,
        principal: &str,
        workspace_limits: Option<crate::RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        Self::require_actor(principal)?;
        let (root, projected) = Self::recovery_projection(root, workspace_limits)?;
        let pending = startup::prepare_resume(&root, &projected, operation)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "no previously selected Backup final activation exists for this operation",
            )
        })?;
        pending.require_owners()?;
        let admitted = pending.admission();
        if admitted.first().result().operation_id != operation
            || admitted
                .published()
                .state()
                .operation_principals
                .get(&operation)
                .map(String::as_str)
                != Some(principal)
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "Backup cold resume differs from its original native operation or principal",
            ));
        }
        let system = DiskRoot::open(&root)?;
        let (selection, bytes) = selected(&system, admitted)?;
        require_initial_targets(&system, admitted, &selection, &AtomicBool::new(false))?;
        let objects = original_objects::verify_recovered(
            admitted.held_roots(),
            admitted.first(),
            admitted.source(),
            &selection.original_objects,
            &AtomicBool::new(false),
        )?;
        // Any completion follows ALL exchanges. Missing native data is then a
        // concrete fault, not authority to fall back to the old seal pair.
        let mut completed = preparation_path_exists(&system, &system.path().join(COMPLETE))?;
        for (_, root) in pending.held_roots() {
            completed |= preparation_path_exists(root, &root.path().join(COMPLETE))?;
        }
        pending.require_owners()?;
        roots::recover_originals(
            admitted.held_roots(),
            admitted.first(),
            admitted.config(),
            &selection.original_roots,
            &bytes,
            completed,
        )?;
        let complete = bounded(&Completion {
            format: "zxs-backup-serving-completed".into(),
            selection_hash: blake3::hash(&bytes).to_hex().to_string(),
            original_objects: objects,
            native_files: roots::completed_files(admitted.held_roots(), admitted.config())?,
        })?;
        // Only absent completion copies may be restored, after the whole actual
        // set was read back. Existing different bytes are never overwritten.
        for (_, root) in admitted.held_roots() {
            install_preparation_bytes(root, &root.path().join(COMPLETE), &complete)?;
        }
        install_preparation_bytes(&system, &system.path().join(COMPLETE), &complete)?;
        completion(&system, admitted, &bytes)?;
        let barrier = barrier(admitted, &selection)?;
        pending.require_owners()?;
        let mut engine = pending.publish(&barrier)?.into_engine()?;
        let warnings = engine.prepare_generation_resources()?;
        let mut prepared = engine.prepare_recovery_with_warnings(warnings)?;
        engine.preflight_workspace_receipts(&mut prepared)?;
        engine.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
        engine.complete_backup_serving_open()?;
        engine.load_workspaces()?;
        engine.verify_selected_move_prepare()?;
        if !engine.is_read_only() && engine.history_move_choice.is_none() {
            engine.restore_published_disk_histories();
        }
        Ok(engine)
    }
}
