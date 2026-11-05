//! Readmission activation and exact SYSTEM-plan crash resumption. Ordinary open
//! does not invent a missing locator event or rediscover a disabled directory.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::{
    runtime::{management_error, open_store},
    store::PreparedReadmission,
};
use crate::engine::membership_transition::archive::persistent::PendingControlArchive;
use crate::engine::membership_transition::readmission::{AdmittedReadmission, request};

const PLAN: &str = "readmission-activation.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ActivationPlan {
    authority: PortableAuthority,
    locator: PreparedReadmission,
}

fn read_plan(root: &Path) -> Result<Option<ActivationPlan>> {
    open_optional_regular(&root.join(PLAN))?
        .map(|file| {
            let bytes = read_file_bytes(file, MAX_METADATA)?;
            let plan: ActivationPlan = decode_strict(&bytes)?;
            if plan.authority.readmission_core().is_none() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "readmission plan has another authority kind",
                ));
            }
            plan.authority.intent()?;
            Ok(plan)
        })
        .transpose()
}

fn generation(
    portable: PortableAuthority,
    source: &PendingControlArchive,
) -> Result<Arc<Generation>> {
    let intent = portable.intent()?;
    let (base, history) = source.generation_base(&intent)?;
    portable.verify_registry_source(&base)?;
    let generation = Arc::new(Generation {
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
    PreparedMetadataObject::new(
        "membership-generation-bundle",
        &generation.portable,
        MAX_METADATA,
    )?;
    Ok(generation)
}

impl Engine {
    /// Inspect SYSTEM only. A false result means no durable activation plan;
    /// every malformed or unreadable plan remains an error, never a fallback.
    pub fn readmission_recovery_pending(root: &Path) -> Result<bool> {
        Ok(read_plan(root)?.is_some())
    }

    pub(in crate::engine) fn retire_published_readmission_plan(&self) -> Result<()> {
        if let Some(plan) = read_plan(&self.root)?
            && self
                .state
                .operations
                .contains_key(&plan.authority.intent()?.operation_id)
        {
            self.retire_completed_readmission_plan()?;
        }
        Ok(())
    }

    pub(in crate::engine::membership_transition) fn retire_completed_readmission_plan(
        &self,
    ) -> Result<()> {
        let Some(plan) = read_plan(&self.root)? else {
            return Ok(());
        };
        let intent = plan.authority.intent()?;
        let Some(result) = self.state.operations.get(&intent.operation_id) else {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "an earlier readmission activation plan is still unpublished",
            ));
        };
        let hash = plan.authority.hash()?;
        let in_history = self
            .config
            .generation()
            .map(|generation| generation.portable.contains(&hash))
            .transpose()?
            .unwrap_or(false);
        if result.kind != "disk-readmit"
            || result.request_digest != object_hash(INTENT_KIND, &intent)?
            || !in_history
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "readmission plan is not in published current history",
            ));
        }
        DiskRoot::open(&self.root)?.remove_file_if_present_synced(&self.root.join(PLAN))?;
        Ok(())
    }
    pub(in crate::engine::membership_transition) fn install_readmission_generation(
        &mut self,
        portable: PortableAuthority,
        source: &PendingControlArchive,
        target_root: DiskRoot,
        target_writer: Arc<FenceOwner>,
    ) -> Result<OperationResult> {
        let generation = generation(portable, source)?;
        let request = request(&generation.intent)?;
        generation
            .portable
            .prepare_namespace_activation(&generation.base, self.current_namespaces.as_deref())?;
        let target_id = request.target.disk_id.clone();
        let admission = AdmittedReadmission::verify(&generation.portable, |disk| {
            if disk.disk_id == target_id {
                Ok(&target_root)
            } else {
                Ok(&self.disk_resources.get(disk)?.root)
            }
        })?;
        crate::engine::membership_transition::readmission::persist_probe_admission(
            &self.root,
            &generation.portable,
            &admission,
        )?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_readmission_probe_admission")?;
        let mut store = open_store(&self.root)?;
        let plan = match read_plan(&self.root)? {
            Some(plan) if plan.authority.hash()? == generation.hash => {
                store
                    .readmission_source(&plan.locator)
                    .map_err(management_error)?;
                plan
            }
            previous => {
                if let Some(previous) = previous {
                    let old = previous.authority.intent()?;
                    if self
                        .state
                        .operations
                        .get(&old.operation_id)
                        .is_none_or(|result| result.kind != "disk-readmit")
                        || !generation.portable.contains(&previous.authority.hash()?)?
                    {
                        return Err(Error::new(
                            ErrorCode::RecoveryRequired,
                            "another readmission plan is unresolved",
                        ));
                    }
                }
                let previous = store
                    .reload(&self.config.pool_id)
                    .map_err(management_error)?;
                let plan = ActivationPlan {
                    authority: generation.portable.clone(),
                    locator: store
                        .prepare_readmission(&previous, &admission)
                        .map_err(management_error)?,
                };
                PreparedMetadataObject::new(
                    "disk-readmission-activation-plan",
                    &plan,
                    MAX_METADATA,
                )?;
                write_json(&self.root.join(PLAN), &plan, true)?;
                plan
            }
        };
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_readmission_activation_plan")?;
        let locator = store
            .resume_readmission(&plan.locator, &admission)
            .map_err(management_error)?;
        store.sync_current(&locator).map_err(management_error)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_readmission_locator")?;
        let config = generation
            .intent
            .target
            .clone()
            .with_workspace_limits(Some(self.config.effective_workspace_limits()))?
            .with_generation(Arc::clone(&generation))?;
        let config = generation.portable.restrict_io(config)?;
        let config = locator.restrict_config(config, &generation.intent.target)?;
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.disk_id == target_id)
            .expect("invariant: readmission target retained");
        let seal = identity_at(&config, disk, &target_root.directory(target_root.path())?)?;
        let handle = Arc::new(DiskHandle::new(target_root, seal, &config)?);
        let writer = DiskWriter::adopt(&handle.root, target_writer)?;
        let mut handles = self
            .disk_resources
            .handles()
            .map(Arc::clone)
            .collect::<Vec<_>>();
        handles.push(handle);
        let resources = DiskResources::generation_handles(&config, handles)?;
        self._locks.insert(target_id, writer);
        self.config = config;
        self.disk_resources = Arc::new(resources);
        self.current_namespaces = None;
        self.workspace_namespace_baselines.clear();
        self.membership_fence = None;
        self.pending_membership = None;
        self.journals.clear();
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.next_slot = generation.first_sequence()?;
        self.blocked = true;
        self.release_readmitted_exclusion(&admission)?;
        drop(store);
        self.promote_readmission_resources()?;
        let mut prepared = self.prepare_recovery()?;
        self.preflight_workspace_receipts(&mut prepared)?;
        self.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
        self.complete_generation_activation()
    }

    pub(in crate::engine::membership_transition) fn promote_readmission_resources(
        &mut self,
    ) -> Result<()> {
        let generation = self
            .config
            .generation()
            .expect("invariant: readmission generation")
            .clone();
        let core = generation
            .portable
            .readmission_core()
            .expect("invariant: readmission dispatch");
        if self.disk_resources.handles().count() != self.config.io_disks().count() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "readmission promotion requires every admitted target directory",
            ));
        }
        AdmittedReadmission::verify(&generation.portable, |disk| {
            Ok(&self.disk_resources.get(disk)?.root)
        })?;
        let first = self
            .config
            .metadata_members()
            .find(|disk| {
                disk.disk_id
                    != request(&generation.intent)
                        .expect("invariant: readmission")
                        .target
                        .disk_id
            })
            .expect("invariant: readmission retains the nonempty original metadata group")
            .clone();
        let transition = object_hash(INTENT_KIND, &generation.intent)?;
        let source = self.disk_resources.get(&first)?.root.private_child(
            &member_directory(&first, &transition).join(CONTROL_ARCHIVE_DIRECTORY),
        )?;
        let source = PendingControlArchive::from_installed(
            source,
            &generation.portable.archive_hash()?,
            &transition,
        )?;
        for disk in self.config.io_disks().cloned().collect::<Vec<_>>() {
            let root = &self.disk_resources.get(&disk)?.root;
            self._locks.verify_disk(&disk, root)?;
            let path = member_directory(&disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY);
            root.private_dir(&path)?;
            source.install(root.private_child(&path)?)?;
            install_generation_archive(
                root.private_child(&path)?,
                root,
                &disk,
                &generation.portable.archive_hash()?,
                &generation.intent,
            )?;
            root.write_json(
                &disk.root.join("metadata/pool-membership/config.json"),
                &generation.original,
                false,
            )?;
            if disk.metadata_member || disk.disk_id == request(&generation.intent)?.target.disk_id {
                core.verify_member(root, &disk, &generation.hash, true)?;
            }
            publication::install_marker(root, &disk, &generation)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_readmission_marker_{}", disk.disk_id))?;
            let handle = self.disk_resources.get(&disk)?;
            predecessor_seal_matches(
                &self.config,
                &disk,
                &handle.root,
                &handle
                    .root
                    .read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?,
                &handle.seal,
            )?;
            handle
                .root
                .write_json(&disk.root.join("disk.json"), &handle.seal, true)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_readmission_seal_{}", disk.disk_id))?;
        }
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.next_slot = generation.first_sequence()?;
        Ok(())
    }

    /// Resume only the exact durable readmission plan. Its old fixed member
    /// decisions and full frozen publications are independently reread before
    /// the stopped directory is opened or the locator event is advanced.
    pub fn resume_disk_readmission(root: &Path) -> Result<Self> {
        Self::resume_readmission_plan(root, None, None)
    }

    /// Resume the caller's exact saved operation and actor. A different plan
    /// is rejected before any storage root is opened.
    ///
    /// # Errors
    /// Rejects a different operation/actor, missing original fixed witnesses,
    /// an unclosed old lifetime or changed directory identity.
    pub fn resume_disk_readmission_operation(
        root: &Path,
        operation_id: OperationId,
        principal: &str,
        limits: Option<crate::RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        Self::require_actor(principal)?;
        Self::resume_readmission_plan(root, Some((operation_id, principal)), limits)
    }

    fn resume_readmission_plan(
        root: &Path,
        expected: Option<(OperationId, &str)>,
        limits: Option<crate::RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        let (root, projected) = Self::recovery_projection(root, limits)?;
        let plan = read_plan(&root)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "no durable readmission activation plan",
            )
        })?;
        let portable = &plan.authority;
        let intent = portable.intent()?;
        if expected.is_some_and(|(operation, principal)| {
            intent.operation_id != operation || intent.principal != principal
        }) {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "readmission plan belongs to another operation or actor",
            ));
        }
        let core = portable
            .readmission_core()
            .expect("invariant: read plan checked");
        let request = request(&intent)?;
        if !portable.matches_metadata_projection(&projected)? {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "readmission plan differs from projected pool",
            ));
        }
        let mut store = open_store(&root)?;
        let previous = store
            .readmission_source(&plan.locator)
            .map_err(management_error)?;
        let mut original = BTreeMap::new();
        let source_admissions = store.readmission_probe_admissions(&previous)?;
        previous
            .select_readmission_probes(intent.origin.config(), &source_admissions)
            .map_err(management_error)?
            .try_for_each(|probe| -> Result<()> {
                let Some(disk) = intent
                    .origin
                    .config()
                    .metadata_members()
                    .find(|disk| disk.disk_id == probe.disk_id())
                else {
                    return Ok(());
                };
                let held = DiskRoot::open(probe.path())?;
                if pending::admission::verify_named_root(&held)? != probe.expected_identity() {
                    return Err(Error::new(
                        ErrorCode::DiskIdentityMismatch,
                        "readmission source directory was replaced",
                    ));
                }
                core.verify_member(&held, disk, &portable.hash()?, false)?;
                let path = member_directory(disk, &object_hash(INTENT_KIND, &intent)?)
                    .join(CONTROL_ARCHIVE_DIRECTORY);
                let (state, _) = read_generation_base(
                    held.private_child(&path)?,
                    &portable.archive_hash()?,
                    &object_hash(INTENT_KIND, &intent)?,
                    &intent,
                )?;
                core.verify_source(&state)?;
                original.insert(disk.disk_id.clone(), held);
                Ok(())
            })?;
        if original.len() != intent.origin.config().metadata_members().len()
            || original.len() < intent.origin.config().metadata_quorum()
        {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "readmission recovery lacks independent fixed original witnesses",
            ));
        }
        let target = intent
            .target
            .disks
            .iter()
            .find(|disk| disk.disk_id == request.target.disk_id)
            .expect("invariant: target verified");
        previous
            .verify_stopped_directory(
                &request.target,
                &target.root,
                FileIdentity {
                    device: request.directory.device,
                    inode: request.directory.inode,
                },
            )
            .map_err(management_error)?;
        let first = intent
            .origin
            .config()
            .metadata_members()
            .next()
            .expect("invariant: source N is positive");
        let transition = object_hash(INTENT_KIND, &intent)?;
        let source = PendingControlArchive::from_installed(
            original[&first.disk_id].private_child(
                &member_directory(first, &transition).join(CONTROL_ARCHIVE_DIRECTORY),
            )?,
            &portable.archive_hash()?,
            &transition,
        )?;
        let generation = generation(plan.authority.clone(), &source)?;
        let config = portable.restrict_io(
            intent
                .target
                .clone()
                .with_generation(Arc::clone(&generation))?,
        )?;
        // Start with the prior permitted addresses, then add only this verified
        // readmission target. Config cannot silently widen another stopped root.
        let restricted = previous.restrict_config(config.clone(), &intent.target)?;
        let mut allowed = restricted
            .io_disks()
            .map(|disk| disk.disk_id.clone())
            .collect::<BTreeSet<_>>();
        allowed.insert(request.target.disk_id.clone());
        let config = config
            .restrict_io_disks(allowed)?
            .with_workspace_limits(limits)?;
        drop(original);
        let resources = Arc::new(DiskResources::open_activation(&config)?);
        let mut engine = Self::prepare_recovery_resources(root, config, resources)?;
        engine.blocked = true;
        let admission = AdmittedReadmission::verify(portable, |disk| {
            Ok(&engine.disk_resources.get(disk)?.root)
        })?;
        let current = store
            .resume_readmission(&plan.locator, &admission)
            .map_err(management_error)?;
        store.sync_current(&current).map_err(management_error)?;
        drop(store);
        engine.admit_disk_exit_io_exclusion()?;
        engine.promote_readmission_resources()?;
        let mut prepared = engine.prepare_recovery()?;
        engine.preflight_workspace_receipts(&mut prepared)?;
        engine.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
        engine.complete_generation_activation()?;
        engine.restore_current_disk_exit()?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }
}
