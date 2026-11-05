//! All pre-decision work uses the live original authority. The stopped target
//! enters only through the caller's exact directory, after old I/O has drained.
use super::core::Proposal;
use super::*;
use crate::engine::disk_lifecycle::bootstrap::runtime::{management_error, open_store};

impl Engine {
    /// Read the retained registration's original role without opening its root.
    ///
    /// # Errors
    /// Rejects identities that are not currently retained and Disabled, and
    /// registry publication that has not reached its authenticated boundary.
    pub fn preview_disk_readmission_metadata_member(&self, disk_id: &str) -> Result<bool> {
        let generation = self.config.generation().ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "readmission requires a published Disabled generation",
            )
        })?;
        let registry = generation.portable.verified_registry()?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "readmission requires an authenticated registry",
            )
        })?;
        if self
            .current_namespaces
            .as_ref()
            .is_none_or(|view| view.registry() != registry.commitment())
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Disabled publication has not been admitted",
            ));
        }
        let definition = registry
            .historical_topology()
            .find(|disk| disk.identity.disk_id == disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigConflict,
                    "readmission names no historical registration",
                )
            })?;
        if registry.registration(&definition.identity)? != Some(crate::RegistrationMode::Disabled) {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "readmission requires a retained Disabled registration; Detached is permanent",
            ));
        }
        Ok(definition.original_metadata_member)
    }
    /// Fence an explicit Disabled registration for readmission. This does not
    /// yet make the directory writable. The supplied descriptor is consumed.
    ///
    /// # Errors
    /// Rejects Detached/Enabled identities, changed physical directories, live
    /// old I/O tails, missing fixed original witnesses and unresolved publication.
    pub fn begin_disk_readmission(
        &mut self,
        disk_id: &str,
        principal: &str,
        operation_id: OperationId,
        directory: PendingMemberDirectory,
    ) -> Result<DiskReadmissionStatus> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        let phase = if self.state.operations.contains_key(&operation_id) {
            DiskReadmissionPhase::Published
        } else {
            DiskReadmissionPhase::Activating
        };
        if let Some(status) = self
            .config
            .generation()
            .map(|generation| generation.portable.readmission_status(operation_id, phase))
            .transpose()?
            .flatten()
        {
            if status.target.disk_id != disk_id || status.principal != principal {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "readmission operation belongs to another request",
                ));
            }
            return Ok(status);
        }
        self.admit_disk_exit_io_exclusion()?;
        if let Some(fence) = &self.membership_fence {
            let request = request(&fence.intent)?;
            if fence.intent.operation_id != operation_id
                || fence.intent.principal != principal
                || request.target.disk_id != disk_id
            {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "pending readmission differs",
                ));
            }
            let (root, _writer) = hold_directory(&fence.intent, directory)?;
            request.directory.verify_physical_root(&root)?;
            self.persist_membership_fence()?;
            return self.disk_readmission_status()?.ok_or_else(|| {
                Error::new(ErrorCode::RecoveryRequired, "readmission fence disappeared")
            });
        }
        self.require_write()?;
        if self.state.operations.contains_key(&operation_id) {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "readmission operation is already published",
            ));
        }
        let generation = self.config.generation().ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "readmission requires a published Disabled generation",
            )
        })?;
        let registry = generation.portable.verified_registry()?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "readmission requires an authenticated registry",
            )
        })?;
        if self
            .current_namespaces
            .as_ref()
            .is_none_or(|view| view.registry() != registry.commitment())
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Disabled publication has not been admitted",
            ));
        }
        let definition = registry
            .historical_topology()
            .find(|disk| disk.identity.disk_id == disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigConflict,
                    "readmission names no historical registration",
                )
            })?;
        let identity = definition.identity.clone();
        let metadata = definition.original_metadata_member;
        if registry.registration(&identity)? != Some(crate::RegistrationMode::Disabled) {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "readmission requires a retained Disabled registration; Detached is permanent",
            ));
        }
        DiskIoBinding::require_closed_lifetimes(&identity)?;
        let disk = self
            .config
            .disks
            .iter()
            .find(|disk| disk.disk_id == disk_id)
            .expect("invariant: registry configuration was authenticated");
        if directory.disk_id != disk_id {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "readmission descriptor names another registration",
            ));
        }
        let root = DiskRoot::from_directory(disk.root.clone(), directory.directory)?;
        let physical = pending::admission::verify_named_root(&root)?;
        let original_seal =
            decode_seal(&root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?)?;
        let mut expected = identity_at(&self.config, disk, &root.directory(root.path())?)?;
        expected.format = original_seal.format.clone();
        validate_stored_seal(&original_seal, &expected)?;
        let store = open_store(&self.root)?;
        let locator = store
            .reload(&self.config.pool_id)
            .map_err(management_error)?;
        locator
            .verify_stopped_directory(&identity, &disk.root, physical)
            .map_err(management_error)?;
        if locator
            .recorded_admission_epoch(&identity)
            .map_err(management_error)?
            != self.config.io_admission_epoch(disk_id)?
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "readmission locator closes another epoch",
            ));
        }
        drop(store);
        let (_, stopped_operation, stopped_transition) = generation
            .portable
            .bootstrap_stops()?
            .into_iter()
            .find(|(target, _, _)| target == &identity)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "readmission has no authenticated current stop",
                )
            })?;
        let mut target = (*self.config).clone();
        target
            .disks
            .iter_mut()
            .find(|disk| disk.disk_id == disk_id)
            .expect("invariant: target disk exists")
            .metadata_member = metadata;
        let target = ValidatedConfig::try_from(target)?
            .with_workspace_limits(Some(self.effective_workspace_limits()))?;
        let request = ReadmissionRequest {
            target: identity,
            directory: pending::admission::member_identity(&self.config, disk, physical),
            original_seal,
            predecessor_registry: registry.commitment().clone(),
            stopped_operation,
            stopped_transition,
            previous_admission: self.config.io_admission_epoch(disk_id)?,
            role: if metadata {
                ReadmissionRole::Metadata
            } else {
                ReadmissionRole::DataOnly
            },
        };
        self.retire_completed_readmission_plan()?;
        let members = self.members();
        self.validate_fence_members(&members)?;
        if self.journals.values().any(JournalWriter::needs_recovery) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "readmission requires clean original journals",
            ));
        }
        let intent = TransitionIntent {
            operation_id,
            principal: principal.into(),
            origin: Origin::Readmission {
                config: self.config.clone(),
                predecessor_authority_hash: generation.hash.clone(),
                request: Box::new(request),
            },
            target,
            added_roots: BTreeMap::new(),
            effect: None,
            fence_scope: None,
            boundary: self.handoff_boundary(&members)?,
            fence_sequence: self.next_slot,
        };
        intent.verify_target()?;
        let next_slot = self.next_slot.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimit, "readmission sequence exhausted")
        })?;
        let hash = PreparedMetadataObject::new(INTENT_KIND, &intent, MAX_METADATA)?.hash;
        self.blocked = true;
        self.metadata_integrity.invalidate();
        self.published_state_base = None;
        self.next_slot = next_slot;
        self.membership_fence = Some(AdmittedFence {
            intent,
            hash,
            receipts: BTreeMap::new(),
            prepared_roots: BTreeMap::new(),
        });
        self.persist_membership_fence()?;
        self.disk_readmission_status()?
            .ok_or_else(|| Error::new(ErrorCode::RecoveryRequired, "readmission fence disappeared"))
    }

    /// Install independent control copies and immutable decisions, then publish
    /// the prescribed readmission. Retries use the same archive and descriptor.
    /// No payload is erased, copied as a recovery backup or relabelled as new.
    pub fn activate_disk_readmission(
        &mut self,
        source_archive: &Path,
        expected_archive_root: &str,
        directory: PendingMemberDirectory,
    ) -> Result<OperationResult> {
        self.require_no_commit_preparation()?;
        if self
            .config
            .generation()
            .is_some_and(|generation| generation.portable.readmission_core().is_some())
            && self.membership_fence.is_none()
        {
            let generation = self
                .config
                .generation()
                .expect("invariant: readmission generation checked");
            if generation.portable.archive_hash()? != expected_archive_root {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "readmission archive belongs to another operation",
                ));
            }
            drop(directory);
            if !self.generation_activation_published() {
                self.promote_readmission_resources()?;
                let mut prepared = self.prepare_recovery()?;
                self.preflight_workspace_receipts(&mut prepared)?;
                // Rebuild the write gate from admitted journals, not the failed
                // attempt. A failed replay must keep this live Engine fenced.
                self.blocked = false;
                if let Err(error) =
                    self.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)
                {
                    self.blocked = true;
                    return Err(error);
                }
                if !self.generation_activation_published() {
                    // An unpublished generation keeps its activation barrier
                    // until complete_generation_activation persists it.
                    self.blocked = true;
                }
            }
            return self.complete_generation_activation();
        }
        let fence = self
            .membership_fence
            .as_ref()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "readmission activation has no original fence",
                )
            })?
            .clone();
        request(&fence.intent)?.verify_target(&fence.intent)?;
        let source = PendingControlArchive::open(self, source_archive, expected_archive_root)?;
        let (root, writer) = hold_directory(&fence.intent, directory)?;
        let target = fence
            .intent
            .target
            .disks
            .iter()
            .find(|disk| {
                disk.disk_id
                    == request(&fence.intent)
                        .expect("invariant: readmission checked")
                        .target
                        .disk_id
            })
            .expect("invariant: readmission target exists");
        for disk in fence.intent.origin.config().metadata_members() {
            install_source(&source, &self.disk_resources.get(disk)?.root, disk, &fence)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_readmission_archive_{}", disk.disk_id))?;
        }
        install_source(&source, &root, target, &fence)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_readmission_target_archive")?;
        let proposal = Proposal::prepare(self, &source)?;
        let target_decision = proposal.choose(&root, target)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_readmission_target_decision")?;
        let mut decisions = BTreeMap::new();
        for disk in fence.intent.origin.config().metadata_members() {
            let decision = proposal.choose(&self.disk_resources.get(disk)?.root, disk)?;
            decisions.insert(disk.disk_id.clone(), decision);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_readmission_decision_{}", disk.disk_id))?;
        }
        let core = ReadmissionCore::finish(proposal, decisions, target_decision)?;
        let portable = PortableAuthority::new_readmission(
            core,
            &self
                .config
                .generation()
                .expect("invariant: readmission source generation")
                .portable,
        )?;
        self.install_readmission_generation(portable, &source, root, writer)
    }

    pub fn disk_readmission_status(&self) -> Result<Option<DiskReadmissionStatus>> {
        if let Some(fence) = &self.membership_fence {
            return if matches!(fence.intent.origin, Origin::Readmission { .. }) {
                status(
                    &fence.intent,
                    DiskReadmissionPhase::Fenced,
                    self.state.topology_generation,
                )
                .map(Some)
            } else {
                Ok(None)
            };
        }
        let Some(generation) = self.config.generation() else {
            return Ok(None);
        };
        if generation.portable.readmission_core().is_none() {
            return Ok(None);
        }
        let phase = if self.generation_activation_published()
            && self
                .current_namespaces
                .as_ref()
                .is_some_and(|view| Some(view.registry()) == generation.portable.exit_registry())
        {
            DiskReadmissionPhase::Published
        } else {
            DiskReadmissionPhase::Activating
        };
        status(
            &generation.portable.intent()?,
            phase,
            generation.base.topology_generation,
        )
        .map(Some)
    }
}

pub(in crate::engine::membership_transition) fn status(
    intent: &TransitionIntent,
    phase: DiskReadmissionPhase,
    source_topology_generation: u64,
) -> Result<DiskReadmissionStatus> {
    let request = request(intent)?;
    let source: &Config = intent.origin.config();
    let boundary = intent.boundary.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "readmission source boundary is missing",
        )
    })?;
    let root = source
        .disks
        .iter()
        .find(|disk| disk.disk_id == request.target.disk_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "readmission source target is missing",
            )
        })?;
    let authority = intent.origin.predecessor().ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "readmission source authority is missing",
        )
    })?;
    Ok(DiskReadmissionStatus {
        operation_id: intent.operation_id,
        principal: intent.principal.clone(),
        transition_hash: object_hash(INTENT_KIND, intent)?,
        target: request.target.clone(),
        phase,
        metadata_member: request.role == ReadmissionRole::Metadata,
        source_configuration_digest: blake3::hash(&json_codec::to_vec(source)?)
            .to_hex()
            .to_string(),
        source_root_path: root.root.clone(),
        source_membership_authority_hash: authority.to_owned(),
        source_registry_root_hash: request.predecessor_registry.root_hash().to_owned(),
        source_registry_generation: request.predecessor_registry.view_generation(),
        source_sequence: boundary.sequence,
        source_topology_generation,
        source_snapshot_hash: boundary.snapshot_hash.clone(),
    })
}

fn hold_directory(
    intent: &TransitionIntent,
    directory: PendingMemberDirectory,
) -> Result<(DiskRoot, Arc<FenceOwner>)> {
    let request = request(intent)?;
    if directory.disk_id != request.target.disk_id {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "readmission descriptor identifies another directory",
        ));
    }
    DiskIoBinding::require_closed_lifetimes(&request.target)?;
    let disk = intent
        .target
        .disks
        .iter()
        .find(|disk| disk.disk_id == request.target.disk_id)
        .expect("invariant: readmission target verified");
    let root = DiskRoot::from_directory(disk.root.clone(), directory.directory)?;
    request.directory.verify_physical_root(&root)?;
    let directory_lock = root.directory(root.path())?;
    pending::reservation::lock(&directory_lock)?;
    let writer = root.lock_file()?;
    pending::reservation::lock(&writer)?;
    let writer = FenceOwner::new(writer, Some(directory_lock));
    pending::reservation::verify_writer(&root, &writer)?;
    let seal = decode_seal(&root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?)?;
    validate_stored_seal(&seal, &request.original_seal)?;
    Ok((root, writer))
}

fn install_source(
    source: &PendingControlArchive,
    root: &DiskRoot,
    disk: &DiskConfig,
    fence: &AdmittedFence,
) -> Result<()> {
    let path =
        pending::member_directory(disk, &fence.hash).join(pending::CONTROL_ARCHIVE_DIRECTORY);
    root.private_dir(&path)?;
    source.install(root.private_child(&path)?)?;
    source.install_generation_objects(root, disk, fence)
}
