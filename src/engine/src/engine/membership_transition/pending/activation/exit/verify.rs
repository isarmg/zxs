//! The final admission barrier reads real retained publication bytes and their
//! own journals. Neither a portable certificate nor an in-memory result suffices.
use super::runtime::ExitIoOwner;
use super::*;
use crate::engine::disk_registry::current::PreparedCurrentNamespaces;
use crate::publication_evidence::verified_recovery_snapshot_at;

impl ExitCore {
    pub(in crate::engine::membership_transition) fn current_namespaces(
        &self,
        state: &State,
        previous: Option<&crate::engine::disk_registry::current::CurrentNamespaces>,
    ) -> Result<PreparedCurrentNamespaces> {
        let intent = self.intent()?;
        let boundary = intent
            .boundary
            .as_ref()
            .expect("invariant: verified exit source");
        let predecessor = self.predecessor(&intent)?;
        let successor = self.registry_from_source(state)?;
        let target = &successor
            .pending_exit()
            .expect("invariant: verified exit successor")
            .target;
        let source = match &self.continuation {
            Some(_) => FrozenDetachSource::capture_current(
                state,
                intent.origin.config(),
                &boundary.snapshot_hash,
                intent
                    .origin
                    .predecessor()
                    .expect("invariant: continued registry has parent"),
                previous.ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "continued exit lacks its replayed current namespace source",
                    )
                })?,
                &predecessor,
                target,
            )
            .map_err(projection_error)?,
            None => {
                if previous.is_some() {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "first registry exit cannot discard an inherited namespace",
                    ));
                }
                FrozenDetachSource::capture_state(
                    state,
                    intent.origin.config(),
                    Some(&boundary.snapshot_hash),
                    intent.origin.predecessor(),
                    &predecessor,
                    target,
                )
                .map_err(projection_error)?
            }
        };
        match &successor
            .pending_exit()
            .expect("invariant: verified successor")
            .effect
        {
            RegistryExitEffect::Disable => source
                .prepare_disabled_namespaces(&successor)
                .map_err(projection_error),
            RegistryExitEffect::Detach {
                occurrence_mask_hash,
            } => source
                .verify_mask(occurrence_mask_hash, |kind, hash| {
                    read_objects(&self.objects, kind, hash)
                })
                .map_err(projection_error)?
                .prepare_current_namespaces(&successor)
                .map_err(projection_error),
        }
    }
}

impl Engine {
    /// Establish the runtime prohibition before recovery can append any D3
    /// receipt. Restricted bootstrap excludes names; this separately proves no
    /// older live logical I/O domain remains in this process.
    pub(in crate::engine) fn admit_disk_exit_io_exclusion(&mut self) -> Result<()> {
        let Some(generation) = self.config.generation().cloned() else {
            return Ok(());
        };
        for (target, operation, transition) in generation.portable.bootstrap_stops()? {
            if !self.config.excludes_io_disk(&target.disk_id)
                || self
                    .disk_resources
                    .handles()
                    .any(|handle| handle.seal.disk_id == target.disk_id)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "historical exit target remains in runtime resources",
                ));
            }
            let stop = self.recover_disk_exit_stop(&target, operation, &transition)?;
            if let Some(owner) = self
                .pending_disk_exit
                .as_ref()
                .and_then(|pending| pending.ownership.get(&target.disk_id))
            {
                match owner {
                    ExitIoOwner::Live(binding) => {
                        binding.closed_proof()?;
                    }
                    ExitIoOwner::Recovered(guard) if guard.identity() == &target => continue,
                    ExitIoOwner::Recovered(_) => {
                        return Err(Error::new(
                            ErrorCode::DiskIdentityMismatch,
                            "recovered logical exclusion differs from authenticated history",
                        ));
                    }
                    ExitIoOwner::Unopened => {
                        return Err(Error::new(
                            ErrorCode::RecoveryRequired,
                            "unopened exit target lacks its logical I/O exclusion",
                        ));
                    }
                }
            }
            // Keep the old closed physical binding and every earlier guard held
            // until the process-wide logical exclusion has actually succeeded.
            let recovered = self.recover_disk_exit_io(&stop, &target)?;
            let owner = crate::disk::RecoveredDiskIo::admit(&recovered)?;
            let pending = self
                .pending_disk_exit
                .get_or_insert_with(|| PendingDiskExit {
                    generation: Arc::clone(&generation),
                    ownership: BTreeMap::new(),
                });
            pending
                .ownership
                .insert(target.disk_id, ExitIoOwner::Recovered(owner));
        }
        Ok(())
    }

    pub(in crate::engine) fn disk_exit_io_exclusion_admitted(&self) -> Result<bool> {
        let Some(generation) = self.config.generation() else {
            return Ok(false);
        };
        let stops = generation.portable.bootstrap_stops()?;
        if stops.is_empty() {
            return Ok(false);
        }
        Ok(stops.iter().all(|(identity, _, _)| {
            self.config.excludes_io_disk(&identity.disk_id)
                && !self.disk_resources.handles().any(|handle| handle.seal.disk_id == identity.disk_id)
                && self.pending_disk_exit.as_ref().is_some_and(|pending| {
                    matches!(pending.ownership.get(&identity.disk_id), Some(ExitIoOwner::Recovered(guard)) if guard.identity() == identity)
                })
        }))
    }

    /// Called at the exact D3 replay boundary. Later snapshots must advance the
    /// admitted owned view, never bypass it using a checkpoint or a raw old head.
    pub(in crate::engine) fn restore_current_disk_exit(&mut self) -> Result<()> {
        let result = self.restore_current_disk_exit_inner();
        if result.is_err() {
            // A durable D3 is not permission to serve an unauthenticated view.
            // The publication may already exist even though final admission
            // failed, so keep ordinary work blocked until exact replay succeeds.
            self.blocked = true;
            self.metadata_integrity.invalidate();
        }
        result
    }

    fn restore_current_disk_exit_inner(&mut self) -> Result<()> {
        let Some(generation) = self.config.generation().cloned() else {
            return Ok(());
        };
        if generation.portable.is_backup() {
            return self.restore_backup_current();
        }
        let Some(registry) = generation.registry() else {
            return Ok(());
        };
        let first = generation.first_sequence()?;
        if self.state.sequence < first {
            return Ok(());
        }
        if let Some(view) = &self.current_namespaces {
            return if view.registry() == registry && view.activation_sequence() == first {
                Ok(())
            } else {
                Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "active namespace registry differs from current authority",
                ))
            };
        }
        if self.state.sequence != first {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "registry namespace requires exact activation replay before later snapshots",
            ));
        }
        self.admit_disk_exit_io_exclusion()?;
        let previous = self.replay_registry_source(&generation)?;
        let prepared = generation
            .portable
            .prepare_namespace_activation(&generation.base, previous.as_ref())?;
        let proof = self.verify_disk_exit_d3(&generation)?;
        self.install_current_namespaces(prepared, &proof)
    }

    fn verify_disk_exit_d3(&self, generation: &Generation) -> Result<AdmittedDiskExitPublication> {
        for (identity, operation, transition) in generation.portable.bootstrap_stops()? {
            let stop = self.recover_disk_exit_stop(&identity, operation, &transition)?;
            if !stop.matches(&identity, operation, &transition)
                || !self.config.excludes_io_disk(&identity.disk_id)
                || self.disk_resources.handles().any(|handle| handle.seal.disk_id == identity.disk_id)
                || !self.pending_disk_exit.as_ref().is_some_and(|pending| {
                    matches!(pending.ownership.get(&identity.disk_id), Some(ExitIoOwner::Recovered(guard)) if guard.identity() == &identity)
                })
            {
                return Err(Error::new(ErrorCode::RecoveryRequired,
                    "registry activation lacks a historical durable stop or logical I/O exclusion"));
            }
        }
        let registry = generation
            .verified_registry()?
            .expect("invariant: registry-bearing activation");
        let expected = generation.result()?;
        let online = self
            .config
            .metadata_members()
            .filter(|disk| self.disk_resources.available(disk).is_some())
            .cloned()
            .collect::<Vec<_>>();
        if online.len() < self.config.metadata_quorum() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "exit publication lacks its unchanged target metadata quorum",
            ));
        }
        let mut candidate = None;
        let mut publication_hash = None;
        let mut candidate_hash = None;
        for disk in &online {
            let root = &self.disk_resources.get(disk)?.root;
            let authority: PortableAuthority =
                root.read_json_observed(&disk.root.join(GENERATION_FILE), &mut |_| Ok(()))?;
            if authority.hash()? != generation.hash {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "retained member has another exit authority",
                ));
            }
            let scan = scan_configured_at(root, disk, &self.config)?;
            let (record, frame_hash) = scan
                .records
                .iter()
                .find(|(record, _)| {
                    record.sequence == expected.sequence
                        && record.record_type == RecordType::Publication
                })
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "retained member has no actual D3 publication journal receipt",
                    )
                })?;
            let found: Candidate = load_object_at(root, disk, "candidate", &record.candidate_hash)?;
            generation.verify_activation_candidate(&found)?;
            let published: Publication =
                load_object_at(root, disk, "publication", &record.object_hash)?;
            let seal: PublicationSeal = root.read_json_observed(
                &publication_seal_path(disk, &record.candidate_hash),
                &mut |_| Ok(()),
            )?;
            verify_publication_seal(&found, &published, &seal)?;
            if published.result != expected
                || published.candidate_hash != record.candidate_hash
                || candidate_hash
                    .as_ref()
                    .is_some_and(|known| known != &record.candidate_hash)
                || publication_hash
                    .as_ref()
                    .is_some_and(|known| known != &record.object_hash)
                || !seal.receipts.iter().any(|receipt| {
                    receipt.disk_id == disk.disk_id
                        && receipt.slot_id == disk.slot_id
                        && receipt.record == *record
                        && receipt.frame_hash == *frame_hash
                })
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "D3 differs from prescribed result or actual retained receipt",
                ));
            }
            candidate = Some(found);
            candidate_hash = Some(record.candidate_hash.clone());
            publication_hash = Some(record.object_hash.clone());
        }
        let candidate = candidate.expect("invariant: fixed target quorum is nonempty");
        let admitted =
            verified_recovery_snapshot_at(&self.disk_resources, &online, &candidate.snapshot_hash)?;
        admitted.for_candidate(&candidate)?;
        if self.snapshot.as_deref() != Some(candidate.snapshot_hash.as_str())
            || self.state.sequence != candidate.sequence
            || prepare_snapshot(&self.state, MAX_METADATA)?.hash != candidate.snapshot_hash
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "visible state is not exact prescribed exit D3",
            ));
        }
        let occurrence_mask_hash = generation.portable.activation_mask_hash()?;
        let boundary = generation
            .intent
            .boundary
            .as_ref()
            .expect("invariant: exit source");
        Ok(AdmittedDiskExitPublication {
            admission: PublicationAdmission::Live,
            registry: registry.commitment().clone(),
            occurrence_mask_hash,
            source_snapshot: boundary.snapshot_hash.clone(),
            source_sequence: boundary.sequence,
            source_business_generation: generation.base.business_generation,
            published_snapshot: candidate.snapshot_hash.clone(),
            published_sequence: candidate.sequence,
        })
    }
}
