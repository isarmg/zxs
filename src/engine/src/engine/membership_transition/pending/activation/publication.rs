//! Explicit generation resource promotion and prescribed publication.
use super::*;

/// Envelope evidence is not generation identity. Equivalent valid witnesses
/// retain the named bytes and repeat both durability barriers.
fn install_envelope(
    root: &DiskRoot,
    path: &Path,
    generation: &Generation,
    advance: bool,
) -> Result<()> {
    if let Some(file) = root.open_optional_read_regular(path)? {
        let bytes = read_file_bytes(file, MAX_METADATA)?;
        let saved: PortableAuthority = decode_strict(&bytes)?;
        let hash = saved.hash()?;
        if hash == generation.hash {
            return root.write_atomic(path, &bytes, false);
        }
        if !advance || Some(hash.as_str()) != generation.intent.origin.predecessor() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "generation marker is not this authority or its exact predecessor",
            ));
        }
        return root.write_json(path, &generation.portable, true);
    }
    match root.write_json(path, &generation.portable, false) {
        Ok(()) => Ok(()),
        Err(error) => {
            // An uncertain create is resolved at the actual immutable name.
            let Some(file) = root.open_optional_read_regular(path)? else {
                return Err(error);
            };
            let bytes = read_file_bytes(file, MAX_METADATA)?;
            let saved: PortableAuthority = decode_strict(&bytes)?;
            if saved.hash()? != generation.hash {
                return Err(error);
            }
            root.write_atomic(path, &bytes, false)
        }
    }
}

pub(in crate::engine::membership_transition) fn install_marker(
    root: &DiskRoot,
    disk: &DiskConfig,
    generation: &Generation,
) -> Result<()> {
    let directory = generation_directory(disk, &generation.hash);
    root.private_dir(&directory)?;
    root.private_dir(&disk.root.join("metadata/pool-membership"))?;
    if (generation.portable.is_abort() || generation.portable.exit_registry().is_some())
        && disk.metadata_member
    {
        let current_marker =
            match root.open_optional_read_regular(&disk.root.join(GENERATION_FILE))? {
                Some(file) => {
                    let saved: PortableAuthority =
                        decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
                    let hash = saved.hash()?;
                    if hash != generation.hash
                        && Some(hash.as_str()) != generation.intent.origin.predecessor()
                    {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "generation marker has an unrelated predecessor",
                        ));
                    }
                    hash == generation.hash
                }
                None => false,
            };
        let seal = decode_seal(&root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?)?;
        let current_seal = matches!(&seal.format, DiskSealFormat::MembershipGeneration { authority_hash } if authority_hash == &generation.hash);
        let journal = directory.join("journal.bin");
        if root.open_optional_read_regular(&journal)?.is_none() {
            if current_marker || current_seal {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "current authority lost its generation journal",
                ));
            }
            root.write_atomic(&journal, &[], false)?;
        }
    }
    for object in generation.portable.node_objects()? {
        object.store_at_root(root, disk)?;
    }
    install_envelope(root, &directory.join("authority.json"), generation, false)?;
    install_envelope(root, &disk.root.join(GENERATION_FILE), generation, true)
}

pub(crate) fn predecessor_seal_matches(
    config: &ValidatedConfig,
    disk: &DiskConfig,
    root: &DiskRoot,
    bytes: &[u8],
    expected: &DiskSeal,
) -> Result<()> {
    let generation = config
        .generation()
        .expect("invariant: generation resource admission");
    let identity = FileIdentity::from_fd(root.directory(root.path())?)?;
    if let Some(dependency) = generation.portable.data_dependency(&disk.disk_id)? {
        dependency.verify_active_seal(expected)?;
        dependency.verify_local(root)?;
    }
    let value: json_codec::Value = decode_strict(bytes)?;
    if value.get("format").and_then(json_codec::Value::as_str) != Some("membership-pending") {
        let stored = decode_seal(bytes)?;
        if stored.format == expected.format {
            if (generation.portable.is_abort() || generation.portable.exit_registry().is_some())
                && disk.metadata_member
                && root
                    .open_optional_read_regular(&log_path(config, disk))?
                    .is_none()
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "active control generation journal is missing",
                ));
            }
            return validate_stored_seal(&stored, expected);
        }
        let mut predecessor = expected.clone();
        predecessor.format = match generation.intent.origin.predecessor() {
            Some(hash) => DiskSealFormat::MembershipGeneration {
                authority_hash: hash.to_owned(),
            },
            None => DiskSealFormat::ManagedDirectory,
        };
        generation
            .intent
            .origin
            .config()
            .disks
            .iter()
            .find(|old| old.disk_id == disk.disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "new member cannot adopt a managed predecessor",
                )
            })?;
        return validate_stored_seal(&stored, &predecessor);
    }
    let pending: PendingDiskSeal = decode_strict(bytes)?;
    if pending.identity != admission::member_identity(config, disk, identity)
        || pending.transition_hash != object_hash(INTENT_KIND, &generation.intent)?
        || pending.target_config_hash
            != object_hash("membership-target", &generation.intent.target)?
        || pending.control_archive_root != generation.portable.archive_hash()?
        || generation
            .intent
            .origin
            .config()
            .disks
            .iter()
            .any(|old| old.disk_id == disk.disk_id)
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "pending predecessor differs from generation",
        ));
    }
    Ok(())
}

impl Engine {
    /// Complete the pending pure device addition. No payload copies or placement
    /// changes are claimed; original routes and every body remain in place.
    pub fn activate_pending_membership(&mut self) -> Result<OperationResult> {
        if self.membership_fence.is_none()
            && self
                .config
                .generation()
                .is_some_and(|generation| generation.portable.is_exit())
        {
            return self.activate_pending_disk_exit();
        }
        if self
            .membership_fence
            .as_ref()
            .is_some_and(|fence| fence.intent.effect.is_some())
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "exit control preparation awaits authenticated registry, namespace projection and persistent I/O closure; activation is not implemented",
            ));
        }
        if self.config.generation().is_some() && self.membership_fence.is_none() {
            if self.generation_activation_published() {
                return self.complete_generation_activation();
            }
            self.promote_generation_resources().map_err(|error| {
                let code = error.code();
                error.context(code, "promote exact generation resources")
            })?;
            let mut prepared = self.prepare_recovery()?;
            self.preflight_workspace_receipts(&mut prepared)?;
            self.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
            return self.complete_generation_activation();
        }
        self.require_no_commit_preparation()?;
        let previous = self.config.generation_hash().map(str::to_owned);
        let discovered = discover(self.config.clone())?;
        if discovered.generation_hash() != self.config.generation_hash() {
            if discovered
                .generation()
                .is_some_and(|generation| generation.portable.is_abort())
            {
                self.resume_abort_generation(discovered)?;
                return self.activate_pending_membership();
            }
            let pending = self.pending_membership.take().ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "prepared generation requires explicit activation recovery",
                )
            })?;
            let result = self.resume_prepared_generation(discovered, &pending);
            if result.is_err() && self.config.generation_hash() == previous.as_deref() {
                self.pending_membership = Some(pending);
            }
            return result;
        }
        self.verify_membership_joint_decision()?;
        let mut pending = self.pending_membership.take().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "activation requires pending resources",
            )
        })?;
        let result = self.activate_pending(&mut pending);
        if result.is_err() && self.config.generation_hash() == previous.as_deref() {
            self.pending_membership = Some(pending);
        }
        result
    }
    fn resume_prepared_generation(
        &mut self,
        discovered: ValidatedConfig,
        pending: &PendingMembership,
    ) -> Result<OperationResult> {
        let generation = discovered
            .generation()
            .expect("invariant: discovered generation");
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: held pending fence");
        if object_hash(INTENT_KIND, &generation.intent)? != fence.hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "prepared authority differs from held fence",
            ));
        }
        let mut handles = self
            .disk_resources
            .handles()
            .map(Arc::clone)
            .collect::<Vec<_>>();
        let mut locks = Vec::new();
        for (disk, added) in pending.added_resources() {
            let root = added.root.retained(disk.root.clone())?;
            let seal = crate::disk::identity_at(&discovered, disk, &root.directory(root.path())?)?;
            let handle = Arc::new(DiskHandle::new(root, seal, &discovered)?);
            locks.push((
                disk.disk_id.clone(),
                crate::disk::DiskWriter::adopt(&handle.root, Arc::clone(&added._lock))?,
            ));
            handles.push(handle);
        }
        self.disk_resources = Arc::new(DiskResources::generation_handles(&discovered, handles)?);
        self._locks.extend(locks);
        self.config = discovered;
        self.current_namespaces = None;
        self.membership_fence = None;
        self.journals.clear();
        self.blocked = true;
        self.activate_pending_membership()
    }
    fn activate_pending(&mut self, pending: &mut PendingMembership) -> Result<OperationResult> {
        let progress = pending
            .joint
            .as_ref()
            .expect("invariant: joint decision verified");
        let status = &progress.status;
        if !status.all_certificates_synchronized() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "activation needs every joint certificate synchronized",
            ));
        }
        let first = &pending.members[0];
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
        let joint = store.portable(
            status
                .certificate_hash
                .as_deref()
                .expect("invariant: complete certificate"),
            &status.proposal_hash,
        )?;
        let registry = match self
            .config
            .generation()
            .map(|generation| generation.portable.verified_registry())
            .transpose()?
            .flatten()
        {
            Some(previous) => {
                if self
                    .current_namespaces
                    .as_ref()
                    .is_none_or(|view| view.registry() != previous.commitment())
                {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "membership successor lacks the admitted source namespace",
                    ));
                }
                let intent = joint.verify()?.intent;
                Some(super::registry::RegistryContinuation::prepare(
                    &previous,
                    &intent,
                    &self.state,
                    &intent.target,
                )?)
            }
            None => None,
        };
        let portable = PortableAuthority::new(
            joint,
            pending.source.generation_root(),
            self.config
                .generation()
                .map(|generation| &generation.portable),
            registry,
        )?;
        let intent = portable.intent()?;
        let (base, history) = pending.source.generation_base(&intent)?;
        let original = portable.genesis_config()?;
        let epoch = portable.epoch()?;
        let generation = Arc::new(Generation {
            hash: portable.hash()?,
            workspace_roots: portable.workspace_root_aliases()?,
            portable,
            intent: intent.clone(),
            original,
            history,
            epoch,
            base: Arc::new(base),
        });
        generation.next_state()?;
        if generation.portable.exit_registry().is_some() {
            generation.portable.prepare_namespace_activation(
                &generation.base,
                self.current_namespaces.as_deref(),
            )?;
        }
        PreparedMetadataObject::new(
            "membership-generation-bundle",
            &generation.portable,
            MAX_METADATA,
        )?;
        let config = intent
            .target
            .clone()
            .with_workspace_limits(Some(self.config.effective_workspace_limits()))?
            .with_generation(Arc::clone(&generation))?;
        let config = generation.portable.restrict_io(config)?;
        let mut handles = self
            .disk_resources
            .handles()
            .map(Arc::clone)
            .collect::<Vec<_>>();
        let mut locks = Vec::new();
        for (disk, added) in pending.added_resources() {
            let root = added.root.retained(disk.root.clone())?;
            let seal = crate::disk::identity_at(&config, disk, &root.directory(root.path())?)?;
            let handle = Arc::new(DiskHandle::new(root, seal, &config)?);
            locks.push((
                disk.disk_id.clone(),
                crate::disk::DiskWriter::adopt(&handle.root, Arc::clone(&added._lock))?,
            ));
            handles.push(handle);
        }
        self.synchronize_bootstrap_roots(&config, &handles)?;
        // A prepared marker reserves this exact authority but never activates a
        // pending seal. Explicit resume can reconstruct missing derived objects.
        for disk in config.io_disks() {
            let root = &handles
                .iter()
                .find(|handle| handle.seal.disk_id == disk.disk_id)
                .expect("invariant: activation owns every root")
                .root;
            admission::verify_named_root(root)?;
            install_marker(root, disk, &generation)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_activation_marker_{}",
                disk.disk_id
            ))?;
        }
        // Replace in-memory handles, never release any predecessor writer lock.
        let resources = DiskResources::generation_handles(&config, handles)?;
        self.config = config;
        self.current_namespaces = None;
        self.disk_resources = Arc::new(resources);
        self._locks.extend(locks);
        self.membership_fence = None;
        self.pending_membership = None;
        self.journals.clear();
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.next_slot = generation.first_sequence()?;
        self.blocked = true;
        self.promote_generation_resources().map_err(|error| {
            let code = error.code();
            error.context(code, "promote exact generation resources")
        })?;
        let mut prepared = self.prepare_recovery()?;
        self.preflight_workspace_receipts(&mut prepared)?;
        self.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
        self.complete_generation_activation()
    }
    pub(super) fn promote_generation_resources(&mut self) -> Result<()> {
        let Some(generation) = self.config.generation().cloned() else {
            return Ok(());
        };
        if generation.portable.is_exit() {
            return self.promote_disk_exit_resources();
        }
        if self.disk_resources.handles().count() != self.config.io_disks().count() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "activation promotion requires every target root",
            ));
        }
        let transition = object_hash(INTENT_KIND, &generation.intent)?;
        if generation.portable.is_abort() {
            self.restore_abort_archives(&generation)?;
        }
        for disk in self.config.io_disks().filter(|disk| !disk.metadata_member) {
            if let Some(dependency) = generation.portable.data_dependency(&disk.disk_id)? {
                let handle = self.disk_resources.get(disk)?;
                dependency.verify_active_seal(&handle.seal)?;
                dependency.verify_local(&handle.root)?;
            }
        }
        // Independently admit every available target's own archive and journal.
        for disk in self.config.metadata_members() {
            let Some(handle) = self.disk_resources.available(disk) else {
                continue;
            };
            let parent = handle
                .root
                .private_child(&member_directory(disk, &transition))?;
            if generation.portable.is_abort() {
                generation
                    .portable
                    .verify_abort_member(&handle.root, disk)?;
                read_generation_base(
                    parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?,
                    &generation.portable.archive_hash()?,
                    &transition,
                    &generation.intent,
                )?;
                continue;
            }
            let verified = generation.portable.add_joint()?.verify()?;
            let identity = &verified.dependencies[&disk.disk_id].enrollment.identity;
            let physical = FileIdentity::from_fd(handle.root.directory(handle.root.path())?)?;
            if identity.device != physical.device || identity.inode != physical.inode {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "active member differs from joint identity",
                ));
            }
            let store = joint::store::JointStore::open(
                &handle.root,
                &parent,
                identity,
                &transition,
                &generation.portable.add_joint()?.proposal_hash,
                false,
            )?;
            store.verify_certificate(
                &generation.portable.add_joint()?.certificate_hash,
                &generation.portable.add_joint()?.proposal_hash,
                disk,
            )?;
            read_generation_base(
                parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?,
                &generation.portable.archive_hash()?,
                &transition,
                &generation.intent,
            )?;
        }
        let first = self
            .config
            .metadata_members()
            .next()
            .expect("invariant: metadata member");
        for disk in self.config.io_disks() {
            let handle = self.disk_resources.get(disk)?;
            if disk.metadata_member
                || generation
                    .intent
                    .origin
                    .config()
                    .disks
                    .iter()
                    .any(|old| old.disk_id == disk.disk_id)
            {
                let source_disk = if disk.metadata_member { disk } else { first };
                let source = self.disk_resources.get(source_disk)?.root.private_child(
                    &member_directory(source_disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY),
                )?;
                install_generation_archive(
                    source,
                    &handle.root,
                    disk,
                    &generation.portable.archive_hash()?,
                    &generation.intent,
                )
                .map_err(|error| {
                    let code = error.code();
                    error.context(
                        code,
                        format!(
                            "install generation control objects for disk {}",
                            disk.disk_id
                        ),
                    )
                })?;
            }
            handle.root.write_json(
                &disk.root.join("metadata/pool-membership/config.json"),
                &generation.original,
                false,
            )?;
            install_marker(&handle.root, disk, &generation)?;
            handle
                .root
                .private_dir(&generation_directory(disk, &generation.hash))?;
        }
        let active_config = self.config.clone();
        for disk in active_config.io_disks() {
            let Some(handle) = self.disk_resources.available(disk) else {
                continue;
            };
            admission::verify_named_root(&handle.root)?;
            let bytes = handle
                .root
                .read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
            predecessor_seal_matches(&self.config, disk, &handle.root, &bytes, &handle.seal)?;
            install_marker(&handle.root, disk, &generation)?;
            handle
                .root
                .private_dir(&generation_directory(disk, &generation.hash))?;
            handle
                .root
                .write_json(&disk.root.join("disk.json"), &handle.seal, true)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_activation_seal_{}",
                disk.disk_id
            ))?;
        }
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.next_slot = generation.first_sequence()?;
        Ok(())
    }
    pub(in crate::engine) fn complete_generation_activation(&mut self) -> Result<OperationResult> {
        let generation =
            self.config.generation().cloned().ok_or_else(|| {
                Error::new(ErrorCode::RecoveryRequired, "no generation authority")
            })?;
        if generation.portable.is_exit() {
            return self.complete_disk_exit_generation();
        }
        self.admit_disk_exit_io_exclusion()?;
        let expected = generation.result()?;
        if let Some(result) = self.state.operations.get(&expected.operation_id) {
            if result != &expected {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "membership operation result differs from authority",
                ));
            }
            let result = result.clone();
            self.restore_current_disk_exit()?;
            write_json(&self.root.join("config.json"), &self.config, true)?;
            return Ok(result);
        }
        if self.disk_resources.handles().count() != self.config.io_disks().count()
            || self.journals.len() != self.config.metadata_members().len()
        {
            self.blocked = true;
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "unfinished activation requires all targets",
            ));
        }
        if self.state.sequence != generation.base.sequence || self.snapshot != generation.snapshot()
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "unpublished generation does not retain its prescribed base",
            ));
        }
        // Cold activation completes before ordinary open builds its disposable
        // index. Admit that projection from the verified frozen State before
        // normal publication updates it; keep update_index's invariant intact.
        if matches!(self.index_health(), crate::engine::IndexHealth::Unbuilt) {
            self.rebuild_index()?;
        }
        for disk in self.config.io_disks() {
            admission::verify_named_root(&self.disk_resources.get(disk)?.root)?;
        }
        let (next, result) = generation.next_state()?;
        self.next_slot = generation.first_sequence()?;
        self.blocked = false;
        if let Err(error) = self.persist(next, Vec::new(), result.clone()) {
            self.blocked = true;
            return Err(error);
        }
        self.restore_current_disk_exit()?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("before_membership_activation_projection")?;
        write_json(&self.root.join("config.json"), &self.config, true)?;
        Ok(result)
    }
}
