//! Retained-only activation. The stopped directory is never used to finish a
//! marker, result, archive or cleanup after its gate begins quiescing.
use super::*;

/// Retains the original physical gate across retries and after completion. A
/// closed descriptor set cannot disappear from the process registry while this
/// Engine still serves the exit generation.
pub(in crate::engine) struct PendingDiskExit {
    pub(super) generation: Arc<Generation>,
    pub(super) ownership: BTreeMap<String, ExitIoOwner>,
}

pub(super) enum ExitIoOwner {
    Live(DiskIoBinding),
    Recovered(crate::disk::RecoveredDiskIo),
    /// No target resource was admitted. A durable stop and process-wide logical
    /// exclusion are still required before this can become Recovered.
    Unopened,
}

impl Engine {
    /// Complete only an explicitly prepared exit. Full old/new control evidence
    /// is installed on retained directories before a durable stop is recorded.
    /// A busy resource tail returns an error and the same operation can resume.
    pub fn activate_pending_disk_exit(&mut self) -> Result<OperationResult> {
        self.require_no_commit_preparation()?;
        if self.membership_fence.is_none()
            && self
                .config
                .generation()
                .is_some_and(|generation| generation.portable.is_exit())
        {
            self.promote_disk_exit_resources()?;
            return self.complete_disk_exit_generation();
        }
        let operation = self
            .membership_fence
            .as_ref()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "exit activation requires its exact fence",
                )
            })?
            .intent
            .operation_id;
        if self
            .pending_disk_exit
            .as_ref()
            .is_none_or(|pending| pending.generation.intent.operation_id != operation)
        {
            self.prepare_disk_exit_generation()?;
        }
        let pending = self
            .pending_disk_exit
            .as_ref()
            .expect("invariant: prepared exit runtime");
        let generation = Arc::clone(&pending.generation);
        let effect = generation
            .intent
            .effect
            .as_ref()
            .expect("invariant: exit effect");
        let target = effect.identity(&generation.intent.origin.config().pool_id);
        let forbidden = generation.portable.forbidden_disk_ids()?;
        let allowed = generation
            .intent
            .target
            .disks
            .iter()
            .filter(|disk| !forbidden.contains(&disk.disk_id))
            .map(|disk| disk.disk_id.clone())
            .collect();
        let config = generation
            .intent
            .target
            .clone()
            .with_workspace_limits(Some(self.config.effective_workspace_limits()))?
            .with_generation(Arc::clone(&generation))?
            .restrict_io_disks(allowed)?;
        let unopened = matches!(
            pending.ownership.get(&target.disk_id),
            Some(ExitIoOwner::Unopened)
        );
        let binding = match pending
            .ownership
            .get(&target.disk_id)
            .expect("invariant: prepared target closure")
        {
            ExitIoOwner::Live(binding) => Some(binding.clone()),
            ExitIoOwner::Unopened => None,
            ExitIoOwner::Recovered(guard) if guard.identity() == &target => None,
            ExitIoOwner::Recovered(_) => {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "recovered exit closure differs from current target",
                ));
            }
        };
        let (identity, stop_operation, stop_transition) = generation
            .portable
            .bootstrap_stops()?
            .into_iter()
            .find(|(identity, _, _)| identity == &target)
            .expect("invariant: current exit has a first durable stop");
        let stop = match &binding {
            Some(_) => {
                self.synchronize_disk_exit_stop(&identity, stop_operation, &stop_transition)?
            }
            None if unopened => {
                self.synchronize_disk_exit_stop(&identity, stop_operation, &stop_transition)?
            }
            None => self.recover_disk_exit_stop(&identity, stop_operation, &stop_transition)?,
        };
        if !stop.matches(&identity, stop_operation, &stop_transition) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "durable exit stop differs",
            ));
        }
        if unopened {
            let recovered = self.recover_disk_exit_io_for_config(&stop, &target, &config)?;
            let owner = crate::disk::RecoveredDiskIo::admit(&recovered)?;
            self.pending_disk_exit
                .as_mut()
                .expect("invariant: prepared exit runtime")
                .ownership
                .insert(target.disk_id.clone(), ExitIoOwner::Recovered(owner));
        }
        drop(stop);
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_disk_exit_stop")?;
        if let Some(binding) = binding {
            // No target control writes remain. Real outstanding owners decide
            // when the phase reaches Closed, not an empty resource map.
            drop(binding.begin_quiesce()?);
            self.pending_membership = None;
            self.journals.remove(&target.disk_id);
            let closed = binding.closed_proof()?;
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_disk_exit_io_closed")?;
            self.disk_resources = Arc::new(self.disk_resources.excluding_closed(&closed)?);
            self._locks = self._locks.excluding_closed(&closed)?;
        } else {
            self.pending_membership = None;
        }
        let handles = self.disk_resources.handles().map(Arc::clone).collect();
        self.disk_resources = Arc::new(DiskResources::generation_handles(&config, handles)?);
        self.config = config;
        self.membership_fence = None;
        self.journals.clear();
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.current_namespaces = None;
        self.next_slot = generation.first_sequence()?;
        self.blocked = true;
        self.promote_disk_exit_resources()?;
        self.complete_disk_exit_generation()
    }

    pub(super) fn prepare_disk_exit_generation(&mut self) -> Result<()> {
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "exit activation requires its old fence",
            )
        })?;
        if fence.intent.effect.is_none() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "pending handoff is not an exit",
            ));
        }
        self.verify_membership_joint_decision()?;
        let pending = self
            .pending_membership
            .as_ref()
            .expect("invariant: verified joint resources");
        let status = &pending
            .joint
            .as_ref()
            .expect("invariant: verified joint")
            .status;
        if !status.retained_certificates_synchronized() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "exit driver requires complete independent control installation before stopping a root",
            ));
        }
        let first = pending
            .members
            .iter()
            .find(|member| {
                member.disk.metadata_member
                    && self
                        .membership_fence
                        .as_ref()
                        .expect("invariant: exit fence")
                        .intent
                        .target
                        .metadata_members()
                        .any(|target| target.disk_id == member.disk.disk_id)
            })
            .expect("invariant: exit retains a metadata member");
        let parent = first
            .resource
            .root()
            .private_child(&member_directory(&first.disk, &pending.transition_hash))?;
        let store = joint::store::JointStore::open(
            first.resource.root(),
            &parent,
            &first.enrollment.identity,
            &pending.transition_hash,
            &status.proposal_hash,
            false,
        )?;
        let core = ExitCore::prepare(
            self,
            store.portable(
                status
                    .certificate_hash
                    .as_deref()
                    .expect("invariant: complete certificate"),
                &status.proposal_hash,
            )?,
            pending.source.generation_root(),
        )?;
        let portable = PortableAuthority::new_exit(
            core,
            self.config
                .generation()
                .map(|generation| &generation.portable),
        )?;
        let intent = portable.intent()?;
        let (base, history) = pending.source.generation_base(&intent)?;
        let generation = Arc::new(Generation {
            backup_continuation: None,
            hash: portable.hash()?,
            workspace_roots: portable.workspace_root_aliases()?,
            original: portable.genesis_config()?,
            epoch: portable.epoch()?,
            portable,
            intent,
            history,
            base: Arc::new(base),
        });
        generation.next_state()?;
        generation
            .portable
            .exit_core()
            .expect("invariant: exit authority")
            .registry_from_source(&generation.base)?;
        generation
            .portable
            .exit_core()
            .expect("invariant: exit authority")
            .current_namespaces(&generation.base, self.current_namespaces.as_deref())?;
        let bundle = PreparedMetadataObject::new(
            "membership-generation-bundle",
            &generation.portable,
            MAX_METADATA,
        )?;
        let effect = generation
            .intent
            .effect
            .as_ref()
            .expect("invariant: exit effect");
        let disk = self
            .config
            .disks
            .iter()
            .find(|disk| disk.disk_id == effect.disk_id)
            .expect("invariant: exit target registered");
        let owner = if self.config.excludes_io_disk(&disk.disk_id) {
            let target = effect.identity(&self.config.pool_id);
            if !self.pending_disk_exit.as_ref().is_some_and(|pending| {
                matches!(pending.ownership.get(&disk.disk_id), Some(ExitIoOwner::Recovered(guard)) if guard.identity() == &target)
            }) {
                return Err(Error::new(ErrorCode::RecoveryRequired,
                    "disabled target lacks its retained logical I/O exclusion"));
            }
            None
        } else if (generation.intent.fence_scope.is_some() || !disk.metadata_member)
            && self.disk_resources.available(disk).is_none()
        {
            Some(ExitIoOwner::Unopened)
        } else {
            Some(ExitIoOwner::Live(
                self.disk_resources.get(disk)?.root.io_binding()?.clone(),
            ))
        };
        let config = self.config.clone();
        // Prepare every retained root without touching user bodies. The authority
        // name is separate from the active marker; before stop, ordinary restart
        // can still admit the old fenced generation.
        for disk in config
            .io_disks()
            .filter(|disk| disk.disk_id != effect.disk_id)
        {
            let pending = self
                .pending_membership
                .as_ref()
                .expect("invariant: verified joint resources");
            let fence = self
                .membership_fence
                .as_ref()
                .expect("invariant: exit fence");
            let root = &self.disk_resources.get(disk)?.root;
            admission::verify_named_root(root)?;
            self._locks.verify_disk(disk, root)?;
            let path =
                member_directory(disk, &pending.transition_hash).join(CONTROL_ARCHIVE_DIRECTORY);
            root.private_dir(&path)?;
            pending.source.install(root.private_child(&path)?)?;
            pending
                .source
                .verify_installed(root.private_child(&path)?)?;
            pending
                .source
                .install_generation_objects(root, disk, fence)?;
            root.write_atomic(
                &prepared_authority_path(disk, &pending.transition_hash),
                bundle.bytes(),
                false,
            )?;
            let saved: PortableAuthority = decode_strict(&root.read_bytes(
                &prepared_authority_path(disk, &pending.transition_hash),
                MAX_METADATA,
            )?)?;
            if saved.hash()? != generation.hash {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "prepared retained exit authority differs",
                ));
            }
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_disk_exit_prepared_{}", disk.disk_id))?;
        }
        if owner.is_some()
            && self
                .pending_disk_exit
                .as_ref()
                .is_some_and(|pending| pending.ownership.contains_key(&effect.disk_id))
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "an active exit attempted to replace an existing logical exclusion",
            ));
        }
        let mut ownership = self
            .pending_disk_exit
            .take()
            .map(|pending| pending.ownership)
            .unwrap_or_default();
        if let Some(owner) = owner {
            ownership.insert(effect.disk_id.clone(), owner);
        }
        self.pending_disk_exit = Some(PendingDiskExit {
            generation,
            ownership,
        });
        Ok(())
    }

    /// Used by explicit resume and ordinary recovery of an already stopped root.
    /// All filesystem operations remain within the authenticated retained set.
    pub(in crate::engine) fn promote_disk_exit_resources(&mut self) -> Result<()> {
        let generation = self.config.generation().cloned().ok_or_else(|| {
            Error::new(ErrorCode::RecoveryRequired, "no exit generation to promote")
        })?;
        let core = generation
            .portable
            .exit_core()
            .ok_or_else(|| Error::new(ErrorCode::ConfigConflict, "generation is not an exit"))?;
        core.registry_from_source(&generation.base)?;
        let effect = generation
            .intent
            .effect
            .as_ref()
            .expect("invariant: verified exit");
        if !self.config.excludes_io_disk(&effect.disk_id) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "exit resource set is not persistently restricted",
            ));
        }
        let transition = object_hash(INTENT_KIND, &generation.intent)?;
        let verified = core.joint.verify()?;
        let config = self.config.clone();
        for disk in config.io_disks() {
            let root = &self.disk_resources.get(disk)?.root;
            admission::verify_named_root(root)?;
            self._locks.verify_disk(disk, root)?;
            let path = member_directory(disk, &transition);
            if disk.metadata_member {
                let identity = &verified.dependencies[&disk.disk_id].enrollment.identity;
                let parent = root.private_child(&path)?;
                let store = joint::store::JointStore::open(
                    root,
                    &parent,
                    identity,
                    &transition,
                    &core.joint.proposal_hash,
                    false,
                )?;
                store.verify_certificate(
                    &core.joint.certificate_hash,
                    &core.joint.proposal_hash,
                    disk,
                )?;
            }
            read_generation_base(
                root.private_child(&path.join(CONTROL_ARCHIVE_DIRECTORY))?,
                &generation.portable.archive_hash()?,
                &transition,
                &generation.intent,
            )?;
            publication::sync_retained_genesis_anchor(root, disk, &generation.original)?;
            publication::install_marker(root, disk, &generation)?;
            let bytes = root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
            let expected = identity_at(&self.config, disk, &root.directory(root.path())?)?;
            predecessor_seal_matches(&self.config, disk, root, &bytes, &expected)?;
            root.write_json(&disk.root.join("disk.json"), &expected, true)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_disk_exit_marker_{}", disk.disk_id))?;
        }
        // Explicit cold resume starts with an empty Engine. Re-establish the
        // authenticated frozen boundary before replaying this generation's
        // journal, just as ordinary Add promotion does. The archive is not a
        // replacement for any current journal; those were checked above.
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.next_slot = generation.first_sequence()?;
        Ok(())
    }

    pub(in crate::engine) fn complete_disk_exit_generation(&mut self) -> Result<OperationResult> {
        let generation = self
            .config
            .generation()
            .cloned()
            .ok_or_else(|| Error::new(ErrorCode::RecoveryRequired, "no exit generation"))?;
        self.admit_disk_exit_io_exclusion()?;
        let expected = generation.result()?;
        if let Some(published) = self.state.operations.get(&expected.operation_id) {
            if published != &expected {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "exit operation result differs from authority",
                ));
            }
            self.restore_current_disk_exit()?;
            write_json(&self.root.join("config.json"), &self.config, true)?;
            return Ok(expected);
        }
        let mut prepared = self.prepare_recovery()?;
        self.preflight_workspace_receipts(&mut prepared)?;
        self.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
        if !self.state.operations.contains_key(&expected.operation_id) {
            if self.state.sequence != generation.base.sequence
                || self.snapshot != generation.snapshot()
                || self.journals.len() != self.config.metadata_members().len()
            {
                self.blocked = true;
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "exit has no exact complete target publication boundary",
                ));
            }
            if matches!(self.index_health(), crate::engine::IndexHealth::Unbuilt) {
                self.rebuild_index()?;
            }
            let (next, result) = generation.next_state()?;
            self.next_slot = generation.first_sequence()?;
            self.blocked = false;
            if let Err(error) = self.persist(next, Vec::new(), result) {
                self.blocked = true;
                return Err(error);
            }
        }
        #[cfg(feature = "fault-injection")]
        if let Err(error) = self.failpoint("before_disk_exit_final_admission") {
            self.blocked = true;
            return Err(error);
        }
        self.restore_current_disk_exit()?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("before_disk_exit_config_projection")?;
        write_json(&self.root.join("config.json"), &self.config, true)?;
        Ok(expected)
    }
}

pub(in crate::engine::membership_transition) fn prepared_authority_path(
    disk: &DiskConfig,
    transition: &str,
) -> PathBuf {
    disk.root
        .join("metadata/membership-exits")
        .join(transition)
        .join("authority.json")
}

impl Engine {
    /// The old stop remains durable and its runtime epoch stays tombstoned. Only
    /// its obsolete current-owner slot is retired after the new certified epoch
    /// has obtained its own resource. Other stopped directories keep their guards.
    pub(in crate::engine::membership_transition) fn release_readmitted_exclusion(
        &mut self,
        admission: &crate::engine::membership_transition::readmission::AdmittedReadmission,
    ) -> Result<()> {
        let target = &admission.request().target;
        if self.config.excludes_io_disk(&target.disk_id)
            || self.config.io_admission_epoch(&target.disk_id)?.as_deref()
                != Some(admission.transition())
            || !self
                .disk_resources
                .handles()
                .any(|handle| handle.seal.disk_id == target.disk_id)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "new readmission lifetime has no admitted runtime owner",
            ));
        }
        let Some(pending) = self.pending_disk_exit.as_mut() else {
            return Ok(());
        };
        if let Some(owner) = pending.ownership.get(&target.disk_id) {
            match owner {
                ExitIoOwner::Live(binding) => {
                    binding.closed_proof()?;
                }
                ExitIoOwner::Recovered(guard) if guard.identity() == target => (),
                _ => {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "old readmission lifetime has no closed owner",
                    ));
                }
            }
        }
        pending.ownership.remove(&target.disk_id);
        if pending.ownership.is_empty() {
            self.pending_disk_exit = None;
        }
        Ok(())
    }
}
