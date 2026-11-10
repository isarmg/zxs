//! Durable preparation, decision and publication of one frozen transaction.
use super::{Engine, Prepared};
use crate::journal::RecordType;
use crate::{disk::*, journal::*, *};
use json_codec::json;
use std::collections::BTreeMap;
#[path = "verified_payloads.rs"]
mod verified_payloads;
use verified_payloads::{
    PayloadPreparation, PayloadProofReservation, VerifiedPayloadSet, payload_proof_budget,
};

type ExistingCopies<'a> = Vec<Option<RetainedExisting<'a>>>;

struct RetainedExisting<'a> {
    proof: VerifiedExisting<'a>,
    // The proof's body FD closes before its reservation is returned.
    reservation: PayloadProofReservation,
}

/// A publication receipt, not a hash cache. Only successful decide constructs
/// this token, and reuse requires the exact current State allocation and config.
#[derive(Clone)]
pub(super) struct PublishedStateBase {
    state: std::sync::Weak<State>,
    metadata_epoch: super::provenance::MetadataEpoch,
    snapshot: String,
    sequence: u64,
    config_hash: String,
    config: super::ValidatedConfig,
    roots: crate::metadata_map::MapRoot,
    snapshot_basis: SnapshotReuse,
    history_owners: crate::state_projection::DerivedOwners,
}
impl PublishedStateBase {
    /// Every production publication starts with the planner's completed name
    /// capture. This pure State property needs only the same immutable allocation,
    /// not the disk/configuration authority required for graph reuse below.
    pub(super) fn names_captured_for(&self, state: &std::sync::Arc<State>) -> bool {
        self.state.ptr_eq(&std::sync::Arc::downgrade(state))
    }

    fn matches(&self, engine: &crate::engine::PlanningContext<'_>, config_hash: &str) -> bool {
        !engine.blocked
            && self.metadata_epoch.matches(engine.metadata_integrity)
            && self.names_captured_for(engine.state)
            && engine.snapshot.as_deref() == Some(self.snapshot.as_str())
            && engine.state.sequence == self.sequence
            && self.config_hash == config_hash
            && self.config.same_admission(engine.config)
    }
}

/// Authority for this one preparation, captured from a successful publication
/// and the original live periodic owner's executed scan receipt.
pub(crate) struct PublishedDependencyReuse {
    base: std::sync::Arc<PublishedStateBase>,
    coverage: super::integrity_scrub::ScrubCoveragePermit,
}

impl PublishedDependencyReuse {
    fn admit(self, context: &crate::engine::PlanningContext<'_>, config_hash: &str) -> Result<()> {
        if !context
            .published_state_base
            .is_some_and(|base| std::sync::Arc::ptr_eq(base, &self.base))
            || !self.base.matches(context, config_hash)
            || !self.coverage.admit(context, &self.base.metadata_epoch)
        {
            return Err(Error::new(
                ErrorCode::PreparationStale,
                "published dependency authority changed before decision admission",
            )
            .retryable());
        }
        Ok(())
    }

    fn matches(&self, context: &crate::engine::PlanningContext<'_>, config_hash: &str) -> bool {
        context
            .published_state_base
            .is_some_and(|base| std::sync::Arc::ptr_eq(base, &self.base))
            && self.base.matches(context, config_hash)
            && self.coverage.matches(context)
    }

    pub(crate) fn history_basis(&self) -> &crate::metadata_map::MapRoot {
        &self.base.roots
    }
}

/// An owned preparation, never serializable or constructible by callers.
pub(crate) struct DetachedPublication {
    next: State,
    next_current_namespaces: Option<std::sync::Arc<super::CurrentNamespaces>>,
    candidate: Candidate,
    result: OperationResult,
    verified: VerifiedPayloadSet,
    names: crate::container_storage::PreparedNames,
    metadata_epoch: super::provenance::MetadataEpoch,
    prepared_epoch: super::provenance::MetadataEpoch,
    reused_dependencies: Option<PublishedDependencyReuse>,
    base_config: super::ValidatedConfig,
    disk_resources: std::sync::Arc<DiskResources>,
    base_root: std::path::PathBuf,
    base_snapshot: Option<String>,
    base_sequence: u64,
    base_next_slot: u64,
    config_hash: String,
    snapshot_basis: SnapshotReuse,
    history_basis: crate::metadata_map::MapRoot,
    history_owners: crate::state_projection::DerivedOwners,
}

struct ObjectMoveOutcome {
    state: State,
    operation: OperationId,
}
impl super::move_execution::MoveOutcome for ObjectMoveOutcome {
    fn state(&self) -> &State { &self.state }
    fn result(&self) -> &OperationResult { &self.state.operations[&self.operation] }
    fn into_state(self) -> State { self.state }
}

struct ObjectMoveProof {
    verified: VerifiedPayloadSet,
    prepared_epoch: super::provenance::MetadataEpoch,
    reused_dependencies: Option<PublishedDependencyReuse>,
    config_hash: String,
}
struct ObjectMoveSelection {
    previous_snapshot: Option<String>,
    previous_sequence: u64,
    next_slot: u64,
    candidate_hash: String,
    recovered: bool,
    prepared: bool,
    live: Option<ObjectMoveProof>,
}
impl super::move_execution::MoveSelection<ObjectMoveOutcome> for ObjectMoveSelection {
    fn verify(&self, engine: &Engine, candidate: &Candidate, hash: &str) -> Result<()> {
        let expected_slot = if self.prepared || self.recovered {
            candidate.sequence.checked_add(1).ok_or_else(|| Error::new(ErrorCode::ResourceLimit,
                "move decision sequence exhausted"))?
        } else { self.next_slot };
        if engine.snapshot != self.previous_snapshot || engine.state.sequence != self.previous_sequence
            || candidate.previous_snapshot != self.previous_snapshot
            || candidate.sequence != self.next_slot || engine.next_slot != expected_slot
            || hash != self.candidate_hash
        {
            return Err(Error::new(ErrorCode::PreparationStale,
                "move journal selection no longer extends the original namespace"));
        }
        if let Some(proof) = &self.live {
            proof.verified.validate(&engine.config, candidate)?;
            if object_hash("publication-config", &engine.config)? != proof.config_hash
                || !proof.prepared_epoch.matches(&engine.metadata_integrity)
                || proof.reused_dependencies.as_ref().is_some_and(|reuse|
                    !reuse.matches(&engine.planning_context(), &proof.config_hash))
            {
                return Err(Error::new(ErrorCode::PreparationStale,
                    "move payload and metadata preparation authority changed"));
            }
        }
        Ok(())
    }

    fn prepare(&mut self, engine: &mut Engine, candidate: &Candidate, hash: &str) -> Result<()> {
        engine.next_slot = candidate.sequence.checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "move decision sequence exhausted"))?;
        for disk in engine.members() {
            if self.recovered {
                engine.append_recovered_journal(&disk, RecordType::Prepare,
                    candidate.sequence, hash, &candidate.snapshot_hash)?;
            } else {
                engine.append_journal(&disk, RecordType::Prepare,
                    candidate.sequence, hash, &candidate.snapshot_hash)?;
            }
        }
        self.prepared = true;
        super::provenance::validate_candidate_seals(&engine.config, &engine.disk_resources,
            candidate, &engine.metadata_integrity)?;
        if let Some(proof) = &mut self.live {
            if let Some(reuse) = proof.reused_dependencies.take() {
                reuse.admit(&engine.planning_context(), &proof.config_hash)?;
            } else if !proof.prepared_epoch.matches(&engine.metadata_integrity) {
                return Err(Error::new(ErrorCode::PreparationStale,
                    "move metadata changed after complete dependency preparation"));
            }
        }
        Ok(())
    }
    fn mark_decided(&self, _: &mut Engine) {
        // The common CompletionBinding and original PREPARE retain this choice.
    }
    fn retire(&self, _: &mut Engine,
        _: &super::move_execution::PublishedMove<ObjectMoveOutcome>) -> Result<()> {
        // No object-specific archive exists. finish_move retires the shared
        // binding only after all serving scopes and original history roots.
        Ok(())
    }
    fn clear(self, _: &mut Engine) {}
}

enum Completion<'a> {
    Move {
        names: crate::container_storage::PreparedNames,
        recovered: bool,
    },
    Prepared {
        payloads: &'a VerifiedPayloadSet,
        names: crate::container_storage::PreparedNames,
    },
    Recovery,
}

impl Engine {
    pub(crate) fn persist(
        &mut self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
    ) -> Result<()> {
        self.persist_with_metadata_limit(next, prepared, result, MAX_METADATA)
    }
    pub(crate) fn persist_with_metadata_limit(
        &mut self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<()> {
        let detached = self.prepare_publication(next, prepared, result, maximum)?;
        self.decide_publication(detached).map(|_| ())
    }
    pub(crate) fn prepare_publication(
        &self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<DetachedPublication> {
        self.planning_context()
            .prepare_publication(next, prepared, result, maximum)
    }
    fn require_detached_publication(&self, detached: &DetachedPublication) -> Result<()> {
        if self.blocked
            || self.move_is_retained()
            || !detached.prepared_epoch.matches(&self.metadata_integrity)
            || detached.reused_dependencies.as_ref().is_some_and(|reuse| {
                !reuse.matches(&self.planning_context(), &detached.config_hash)
            })
            || !self.config.same_admission(&detached.base_config)
            || !std::sync::Arc::ptr_eq(&self.disk_resources, &detached.disk_resources)
            || self.root != detached.base_root
            || self.snapshot != detached.base_snapshot
            || self.state.sequence != detached.base_sequence
            || self.next_slot != detached.base_next_slot
            || object_hash("publication-config", &self.config)? != detached.config_hash
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PreparationStale,
                "engine changed during detached publication preparation",
            )
            .retryable());
        }
        Ok(())
    }

    /// Reuse ordinary payload/snapshot preparation, then enter the one strict
    /// move executor. Ordinary decide_publication must not expose this State.
    pub(crate) fn publish_move(&mut self, next: State, prepared: Vec<Prepared>,
        result: OperationResult, retained: Vec<String>) -> Result<OperationResult>
    {
        crate::business::object_move::verify_transition(
            &self.state, &next, self.current_namespaces.as_deref(), &result,
        )?;
        let detached = self.prepare_publication(next, prepared, result, MAX_METADATA)?;
        self.require_detached_publication(&detached)?;
        let DetachedPublication { next, candidate, result, verified, names,
            prepared_epoch, reused_dependencies, config_hash,
            base_snapshot, base_sequence, base_next_slot, .. } = detached;
        let object = PreparedMetadataObject::new("candidate", &candidate, MAX_METADATA)?;
        // A retained selector must always be able to find this complete object.
        // Storing an unselected immutable candidate does not change visibility.
        object.store_at(&self.disk_resources, &self.members())?;
        let hash = object.hash.clone();
        let selection = ObjectMoveSelection {
            previous_snapshot: base_snapshot, previous_sequence: base_sequence,
            next_slot: base_next_slot, candidate_hash: hash.clone(),
            recovered: false, prepared: false,
            live: Some(ObjectMoveProof { verified, prepared_epoch,
                reused_dependencies, config_hash }),
        };
        self.finish_move(candidate, hash,
            ObjectMoveOutcome { state: next, operation: result.operation_id },
            retained, None, false, Some(names), selection)
    }

    pub(in crate::engine) fn recover_selected_object_move(&mut self, candidate: Candidate,
        hash: String, next: &State, existing: Option<Publication>) -> Result<OperationResult>
    {
        let binding = self.move_completion.as_ref()
            .ok_or_else(|| Error::new(ErrorCode::RecoveryRequired,
                "object move recovery lost its original completion binding"))?;
        binding.require_candidate(&candidate, &hash)?;
        let receipt = next.operations.get(&candidate.operation_id)
            .ok_or_else(|| Error::new(ErrorCode::PublicationInvalid, "object move result missing"))?;
        crate::business::object_move::verify_transition(
            &self.state, next, self.current_namespaces.as_deref(), receipt,
        )?;
        let body = crate::business::object_move::read_result(receipt)?;
        let selection = ObjectMoveSelection {
            previous_snapshot: self.snapshot.clone(), previous_sequence: self.state.sequence,
            next_slot: candidate.sequence, candidate_hash: hash.clone(),
            recovered: true, prepared: false, live: None,
        };
        let operation = candidate.operation_id;
        self.finish_move(candidate, hash, ObjectMoveOutcome { state: next.clone(), operation },
            body.retained_version_ids, existing, true, None, selection)
    }

    pub(crate) fn decide_publication(
        &mut self,
        detached: DetachedPublication,
    ) -> Result<OperationResult> {
        self.require_detached_publication(&detached)?;
        let DetachedPublication {
            next,
            next_current_namespaces,
            candidate,
            result,
            verified,
            names,
            snapshot_basis,
            history_basis,
            history_owners,
            config_hash,
            metadata_epoch,
            prepared_epoch,
            reused_dependencies,
            ..
        } = detached;
        verified
            .validate(&self.config, &candidate)
            .map_err(|error| {
                let message = format!("prepared payload identity changed: {error}");
                error
                    .context(ErrorCode::PreparationStale, message)
                    .retryable()
            })?;
        crate::protection::preflight_backup_directories(&self.planning_context(), &result)?;
        // Preparation consumed payload capacity, but concurrent staged uploads
        // may have consumed the metadata reserve without changing State.
        for disk in &self.config.disks {
            if (disk.metadata_member
                || candidate
                    .versions
                    .iter()
                    .any(|evidence| evidence.placement.route_slot_ids.contains(&disk.slot_id)))
                && self.disk_resources.get(disk)?.root.available()?
                    < self.config.metadata_reserve_bytes
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::NoSpace,
                    "metadata reserve was consumed during payload preparation",
                ));
            }
        }
        let members = self.members();
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_payloads")?;
        let candidate_object = PreparedMetadataObject::new("candidate", &candidate, MAX_METADATA)?;
        let hash = candidate_object.hash.clone();
        self.next_slot = candidate.sequence.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "decision sequence exhausted",
            )
        })?;
        #[cfg(feature = "fault-injection")]
        let mut prepared = 0;
        for d in &members {
            candidate_object.store_at(&self.disk_resources, std::slice::from_ref(d))?;
            self.append_journal(
                d,
                RecordType::Prepare,
                candidate.sequence,
                &hash,
                &candidate.snapshot_hash,
            )?;
            #[cfg(feature = "fault-injection")]
            {
                prepared += 1;
                self.failpoint(&format!("after_prepare_{prepared}"))?;
            }
        }
        drop(candidate_object);
        #[cfg(feature = "fault-injection")]
        self.failpoint("before_decision_admission")?;
        // PREPARE can take time after the complete payload check. Check all
        // required held seals once at the decision boundary, before the epoch
        // fence, so an unobserved seal replacement cannot authorize ACCEPT or
        // mint a published dependency base.
        super::provenance::validate_candidate_seals(
            &self.config,
            &self.disk_resources,
            &candidate,
            &self.metadata_integrity,
        )
        .map_err(|error| {
            let message = format!("prepared disk seal changed before decision: {error}");
            error
                .context(ErrorCode::PreparationStale, message)
                .retryable()
        })?;
        if let Some(reuse) = reused_dependencies {
            reuse.admit(&self.planning_context(), &config_hash)?;
        } else if !prepared_epoch.matches(&self.metadata_integrity) {
            // Full installation has its own exact post-store epoch. A shared
            // fault observed during PREPARE wins before decision admission too.
            return Err(Error::new(
                ErrorCode::PreparationStale,
                "metadata changed after complete dependency preparation",
            )
            .retryable());
        }
        // Admission is after every PREPARE and before first ACCEPT. The short
        // schedule/epoch fence has now committed to starting this decision;
        // stop/fault after it cannot cancel publication or release ownership.
        self.blocked = true;
        #[cfg(feature = "fault-injection")]
        let admission = self.failpoint("after_decision_admission");
        #[cfg(not(feature = "fault-injection"))]
        let admission: Result<()> = Ok(());
        let finish = admission.and_then(|()| {
            self.complete_candidate_verified(
                &candidate,
                &hash,
                &next,
                None,
                Completion::Prepared {
                    payloads: &verified,
                    names,
                },
            )
        });
        if let Err(e) = finish {
            self.metadata_integrity.invalidate();
            let message = format!("{e}; query operation {} and recover", result.operation_id);
            return Err(e
                .context(storage_contract::ErrorCode::InDoubt, message)
                .retryable());
        }
        let previous = std::mem::replace(&mut self.state, std::sync::Arc::new(next));
        self.current_namespaces = next_current_namespaces;
        self.history_window_roots = Some(super::history_window::HistoryRootBindings::published(
            &self.state,
            history_basis.clone(),
        ));
        self.snapshot = Some(candidate.snapshot_hash.clone());
        self.update_index(&previous);
        // Only the completed member publication establishes this proof. SQLite is
        // a disposable accelerator, not a dependency of the authoritative closure.
        self.published_state_base = metadata_epoch.matches(&self.metadata_integrity).then(|| {
            std::sync::Arc::new(PublishedStateBase {
                state: std::sync::Arc::downgrade(&self.state),
                metadata_epoch,
                snapshot: candidate.snapshot_hash.clone(),
                sequence: self.state.sequence,
                config_hash,
                config: self.config.clone(),
                roots: history_basis,
                snapshot_basis,
                history_owners,
            })
        });
        self.blocked = false;
        // Projection failure is observable separately, never a false failure
        // or rollback of this already durable authoritative publication.
        self.refresh_published_disk_catalogs(Some(&previous));
        self.advance_published_disk_histories(&previous, &candidate, &hash);
        Ok(result)
    }
    pub(crate) fn prepare_payload(&self, p: &Prepared) -> Result<()> {
        self.planning_context().prepare_payload(p)
    }

    pub(super) fn complete_move_candidate(
        &mut self,
        candidate: &Candidate,
        hash: &str,
        next: &State,
        existing: Option<Publication>,
        names: crate::container_storage::PreparedNames,
        recovered: bool,
    ) -> Result<()> {
        if let Some(publication) = &existing {
            self.require_bound_publication(candidate, hash, publication)?;
            if publication.recovery.is_some()
                || next.operations.get(&candidate.operation_id) != Some(&publication.result)
            {
                return Err(Error::new(ErrorCode::PublicationInvalid,
                    "move publication differs from the selected original outcome"));
            }
            // An existing receipt does not discharge the selected move's
            // serving-scope closure. Install its exact names before resuming
            // the original publication; never recapture today's namespace.
            names.apply()?;
            return self.complete_candidate_verified(
                candidate, hash, next, existing, Completion::Recovery,
            );
        }
        self.complete_candidate_verified(
            candidate,
            hash,
            next,
            existing,
            Completion::Move { names, recovered },
        )
    }

    pub(crate) fn complete_candidate(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        existing: Option<Publication>,
    ) -> Result<()> {
        self.complete_candidate_verified(c, hash, next, existing, Completion::Recovery)
    }
    fn require_bound_publication(
        &self,
        c: &Candidate,
        hash: &str,
        publication: &Publication,
    ) -> Result<()> {
        self.verify_candidate_membership(c)?;
        if publication.candidate_hash != hash
            || publication.result.request_digest != c.request_digest
            || (publication.recovery.is_none()
                && publication.finalize.accepts.len() != c.metadata_disk_ids.len())
            || publication.finalize_receipts.len() != c.metadata_disk_ids.len()
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "stored publication does not bind this candidate",
            ));
        }
        Ok(())
    }

    pub(super) fn complete_published_candidate(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        publication: Publication,
        mut append_receipt: impl FnMut(&mut Engine, &DiskConfig, &str) -> Result<Receipt>,
    ) -> Result<()> {
        self.require_bound_publication(c, hash, &publication)?;
        crate::container_storage::repair_historical_names(
            &self.config,
            &self.disk_resources,
            &self.state,
            next,
        )?;
        let publication_object =
            PreparedMetadataObject::new("publication", &publication, MAX_METADATA)?;
        let pub_hash = publication_object.hash.clone();
        let mut publication_receipts = Vec::new();
        for d in self.members() {
            publication_object.store_at(&self.disk_resources, std::slice::from_ref(&d))?;
            publication_receipts.push(append_receipt(self, &d, &pub_hash)?);
            #[cfg(feature = "fault-injection")]
            if publication.recovery.is_some() {
                self.failpoint(&format!(
                    "after_recovery_publication_{}",
                    publication_receipts.len()
                ))?;
            }
        }
        drop(publication_object);
        let seal = PublicationSeal {
            candidate_hash: hash.into(),
            publication_hash: pub_hash,
            receipts: publication_receipts.clone(),
        };
        verify_publication_seal(c, &publication, &seal)?;
        for d in self.members() {
            self.disk_resources.get(&d)?.root.write_json(
                &publication_seal_path(&d, hash),
                &seal,
                false,
            )?;
        }
        // Historical success is preserved even if bytes were later purged or lost.
        // Reconstruct only its original proof, never claim a newly verified payload.
        for evidence in &c.versions {
            for slot in &evidence.placement.route_slot_ids {
                let d = self.disk(slot)?;
                let Some(handle) = self.disk_resources.available(d) else {
                    continue;
                };
                let dir = version_dir(
                    d,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                match handle.root.directory_identity(&dir) {
                    Ok(_) => {}
                    Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                    | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
                let proof = CommitProof {
                    candidate_hash: hash.into(),
                    finalize: publication.finalize.clone(),
                    version_hash: object_hash("version", &evidence.version)?,
                    disk_id: d.disk_id.clone(),
                    slot_id: slot.clone(),
                    route_slot_ids: evidence.placement.route_slot_ids.clone(),
                };
                let expected = publication
                    .payload_receipts
                    .iter()
                    .find(|r| r.version_id == evidence.version.version_id && r.slot_id == *slot);
                let Some(expected) = expected else {
                    if publication.recovery.is_some() {
                        continue;
                    }
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "stored local receipt missing",
                    ));
                };
                if expected.proof_hash != object_hash("commit-proof", &proof)? {
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "stored local receipt differs",
                    ));
                }
                // The first published placement on this slot owns the root
                // proof. Derive that fact from the replayed prefix, not a
                // possibly corrupt local commit-proof candidate pointer.
                let first_on_slot = self
                    .state
                    .placements
                    .get(&evidence.version.version_id)
                    .is_none_or(|placement| !placement.route_slot_ids.contains(slot));
                let proof_dir = if first_on_slot {
                    dir
                } else {
                    local_proof_directory_at(&handle.root, &dir, hash)?
                };
                repair_local_proof(&handle.root, &proof_dir.join("commit-proof.json"), &proof)?;
                repair_local_proof(
                    &handle.root,
                    &proof_dir.join("publication-proof.json"),
                    &LocalPublicationRef {
                        candidate: c,
                        publication: &publication,
                        commit_proof: &proof,
                        publication_receipts: &publication_receipts,
                    },
                )?;
            }
        }
        Ok(())
    }
    fn complete_candidate_verified(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        existing: Option<Publication>,
        completion: Completion<'_>,
    ) -> Result<()> {
        let (verified, names, append) = match completion {
            Completion::Move { names, recovered } => (
                None,
                Some(names),
                if recovered {
                    Engine::append_recovered_journal
                } else {
                    Engine::append_journal
                }
                    as fn(&mut Engine, &DiskConfig, RecordType, u64, &str, &str) -> Result<Receipt>,
            ),
            Completion::Prepared { payloads, names } => (
                Some(payloads),
                Some(names),
                Engine::append_journal
                    as fn(&mut Engine, &DiskConfig, RecordType, u64, &str, &str) -> Result<Receipt>,
            ),
            Completion::Recovery => (
                None,
                None,
                Engine::append_recovered_journal
                    as fn(&mut Engine, &DiskConfig, RecordType, u64, &str, &str) -> Result<Receipt>,
            ),
        };
        if let Some(publication) = existing {
            return self.complete_published_candidate(
                c,
                hash,
                next,
                publication,
                |engine, d, pub_hash| {
                    append(
                        engine,
                        d,
                        RecordType::Publication,
                        c.sequence,
                        hash,
                        pub_hash,
                    )
                },
            );
        }
        self.verify_candidate_membership(c)?;
        let names = match names {
            Some(names) => names,
            None => {
                let projection =
                    crate::state_projection::ProjectionTransition::new(&self.state, next, None)?;
                crate::container_storage::prepare_recovered_names(
                    &self.config,
                    &self.disk_resources,
                    &projection,
                    MAX_METADATA,
                )?
            }
        };
        if verified.is_none() {
            // Recovered candidates have no live HeldSeal proof. Admit their
            // required seals once before the first recovered decision write.
            super::provenance::validate_candidate_seals(
                &self.config,
                &self.disk_resources,
                c,
                &self.metadata_integrity,
            )?;
        }
        let members = self.members();
        let mut accepts = Vec::new();
        for d in &members {
            accepts.push(append(self, d, RecordType::Accept, c.sequence, hash, hash)?);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_accept_{}", accepts.len()))?;
        }
        let finalize = Finalize {
            candidate_hash: hash.into(),
            accepts,
            recovery_id: None,
        };
        let finalize_object = PreparedMetadataObject::new("finalize", &finalize, MAX_METADATA)?;
        let mut final_receipts = Vec::new();
        for d in &members {
            finalize_object.store_at(&self.disk_resources, std::slice::from_ref(d))?;
            final_receipts.push(append(
                self,
                d,
                RecordType::Finalize,
                c.sequence,
                hash,
                &finalize_object.hash,
            )?);
        }
        drop(finalize_object);
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_finalize")?;
        // Only a decided candidate may rename a published file container.
        // A crash leaves the operation indeterminate and recovery replays it.
        #[cfg(feature = "fault-injection")]
        names.apply_observed(&mut |point| self.failpoint(point))?;
        #[cfg(not(feature = "fault-injection"))]
        names.apply()?;
        let directory_result = next.operations.get(&c.operation_id).ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "decided operation result is absent",
            )
        })?;
        crate::protection::materialize_backup_directories(
            &self.config,
            &self.disk_resources,
            directory_result,
        )?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_container_names")?;
        let mut payload_receipts = Vec::new();
        let mut payload_cursor = verified.map(VerifiedPayloadSet::cursor);
        for evidence in &c.versions {
            for slot in &evidence.placement.route_slot_ids {
                let d = self.disk(slot)?;
                let handle = self.disk_resources.get(d)?;
                let directory = version_dir(
                    d,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                if let Some(cursor) = &mut payload_cursor {
                    cursor.validate_payload(&self.config, &evidence.version, slot)?;
                } else {
                    // Recovery has no live held-inode proof and must re-read all bytes.
                    handle.verify_payload(&directory, &evidence.version)?;
                }
                let proof = CommitProof {
                    candidate_hash: hash.into(),
                    finalize: finalize.clone(),
                    version_hash: object_hash("version", &evidence.version)?,
                    disk_id: d.disk_id.clone(),
                    slot_id: d.slot_id.clone(),
                    route_slot_ids: evidence.placement.route_slot_ids.clone(),
                };
                let proof_dir = local_proof_directory_at(&handle.root, &directory, hash)?;
                handle
                    .root
                    .write_json(&proof_dir.join("commit-proof.json"), &proof, false)?;
                payload_receipts.push(PayloadReceipt {
                    version_id: evidence.version.version_id.clone(),
                    disk_id: d.disk_id.clone(),
                    slot_id: d.slot_id.clone(),
                    proof_hash: object_hash("commit-proof", &proof)?,
                });
            }
        }
        if let Some(cursor) = payload_cursor {
            cursor.finish()?;
        }
        payload_receipts
            .sort_by(|a, b| (&a.version_id, &a.disk_id).cmp(&(&b.version_id, &b.disk_id)));
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_commit_proofs")?;
        let publication = Publication {
            recovery: None,
            candidate_hash: hash.into(),
            finalize,
            finalize_receipts: final_receipts,
            payload_receipts,
            result: next
                .operations
                .get(&c.operation_id)
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "candidate result is absent",
                    )
                })?,
        };
        let publication_object =
            PreparedMetadataObject::new("publication", &publication, MAX_METADATA)?;
        let pub_hash = publication_object.hash.clone();
        let mut publication_receipts = Vec::new();
        for d in &members {
            publication_object.store_at(&self.disk_resources, std::slice::from_ref(d))?;
            publication_receipts.push(append(
                self,
                d,
                RecordType::Publication,
                c.sequence,
                hash,
                &pub_hash,
            )?);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_publication_{}", publication_receipts.len()))?;
        }
        drop(publication_object);
        let seal = PublicationSeal {
            candidate_hash: hash.into(),
            publication_hash: pub_hash,
            receipts: publication_receipts.clone(),
        };
        for d in &members {
            self.disk_resources.get(d)?.root.write_json(
                &publication_seal_path(d, hash),
                &seal,
                false,
            )?;
        }
        for evidence in &c.versions {
            for slot in &evidence.placement.route_slot_ids {
                let d = self.disk(slot)?;
                let directory = version_dir(
                    d,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                let root = &self.disk_resources.get(d)?.root;
                let proof_dir = local_proof_directory_at(root, &directory, hash)?;
                let proof = decode_proof(
                    &root.read_bytes(&proof_dir.join("commit-proof.json"), MAX_METADATA)?,
                )?;
                root.write_json(
                    &proof_dir.join("publication-proof.json"),
                    &LocalPublicationRef {
                        candidate: c,
                        publication: &publication,
                        commit_proof: &proof,
                        publication_receipts: &publication_receipts,
                    },
                    false,
                )?;
            }
        }
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_local_publications")?;
        Ok(())
    }
}

/// Restore only a proof reconstructed from a verified published closure.
/// Conflicting local material is retained under the same version directory;
/// recovery never deletes it or treats it as authority for a different result.
fn repair_local_proof<T: data_serialization::Serialize>(
    root: &DiskRoot,
    path: &std::path::Path,
    value: &T,
) -> Result<()> {
    use platform_io::fs_safety::linux::FileIdentity;
    let expected = encode_metadata(value, MAX_METADATA)?;
    let original = match root.open_read_regular(path) {
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
        | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return root.write_atomic(path, &expected, false);
        }
        Err(error) => return Err(error),
        Ok(file) => file,
    };
    // Fail closed on links/special files and I/O errors. An oversized proof can
    // be retained by rename without allocating or interpreting its whole body.
    match root.read_bytes(path, MAX_METADATA) {
        Ok(bytes) if bytes == expected => return Ok(()),
        Ok(_) => {}
        Err(error) if error.code() == storage_contract::ErrorCode::ResourceLimit => {}
        Err(error) => return Err(error),
    }
    let parent = path.parent().expect("proof parent");
    let quarantine = parent.join("proof-quarantine");
    root.private_dir(&quarantine)?;
    if FileIdentity::from_metadata(&original.metadata()?)
        != FileIdentity::from_metadata(&root.open_read_regular(path)?.metadata()?)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "local proof identity changed before repair",
        ));
    }
    let name = format!(
        "{}-{}",
        path.file_name().expect("proof name").to_string_lossy(),
        new_id("req")?
    );
    let retained = quarantine.join(name);
    root.rename_noreplace_destination_first_synced(path, &retained)?;
    if FileIdentity::from_metadata(&original.metadata()?)
        != FileIdentity::from_metadata(&root.open_read_regular(&retained)?.metadata()?)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "local proof identity changed during repair",
        ));
    }
    root.write_atomic(path, &expected, false)
}

/// Bounded scoped workers are fully joined before returning, even on failure.
/// Detached preparation may run without the engine mutex; decision follows all joins.
fn run_payload_groups<T: Sync>(
    groups: &[Vec<&T>],
    work: impl Fn(&T) -> Result<()> + Sync,
) -> Result<()> {
    const WORKERS: usize = 4;
    struct Queue {
        next: usize,
        closed: bool,
        first_error: Option<(usize, Error)>,
    }
    let queue = std::sync::Mutex::new(Queue {
        next: 0,
        closed: false,
        first_error: None,
    });
    struct ClaimedGroup<'a>(&'a std::sync::Mutex<Queue>);
    impl Drop for ClaimedGroup<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0
                    .lock()
                    .expect("invariant: payload preparation queue poisoned")
                    .closed = true;
            }
        }
    }
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..WORKERS.min(groups.len()))
            .map(|_| {
                scope.spawn(|| {
                    loop {
                        let index = {
                            let mut queue = queue
                                .lock()
                                .expect("invariant: payload preparation queue poisoned");
                            if queue.closed
                                || queue.first_error.is_some()
                                || queue.next == groups.len()
                            {
                                return;
                            }
                            let index = queue.next;
                            queue.next += 1;
                            index
                        };
                        // Close admission during unwinding, after the claim lock was released.
                        let _claimed = ClaimedGroup(&queue);
                        let result = groups[index].iter().try_for_each(|payload| work(payload));
                        let Err(error) = result else {
                            continue;
                        };
                        {
                            let mut queue = queue
                                .lock()
                                .expect("invariant: payload preparation queue poisoned");
                            // Claiming is ordered, completion is not. Earlier claimed files
                            // still finish, preserving index-priority error reporting.
                            if queue
                                .first_error
                                .as_ref()
                                .is_none_or(|(first, _)| index < *first)
                            {
                                queue.first_error = Some((index, error));
                            }
                        }
                        return;
                    }
                })
            })
            .collect();
        let mut panic = None;
        for worker in workers {
            match worker.join() {
                Ok(()) => (),
                Err(payload) => {
                    panic.get_or_insert(payload);
                }
            }
        }
        if let Some(payload) = panic {
            std::panic::resume_unwind(payload);
        }
    });
    queue
        .into_inner()
        .expect("invariant: payload preparation queue poisoned")
        .first_error
        .map_or(Ok(()), |(_, error)| Err(error))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/publication/parallel_preparation_tests.rs"]
mod parallel_preparation_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/publication/cache_budget_tests.rs"]
mod cache_budget_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/publication/history_baseline_tests.rs"]
mod history_baseline_tests;

#[cfg(all(test, feature = "fault-injection"))]
#[path = "../../../../test/engine/unit/engine/publication/omission_tests.rs"]
mod omission_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/publication/io_tail_tests.rs"]
mod io_tail_tests;

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn prepare_publication(
        &self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<DetachedPublication> {
        self.prepare_publication_bounded(next, prepared, result, maximum, maximum)
    }
}

impl crate::engine::PlanningContext<'_> {
    fn prepare_publication_bounded(
        &self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
        maximum: usize,
        cache_maximum: usize,
    ) -> Result<DetachedPublication> {
        crate::business::reject_unclosed_file_history_move(self.state, &next, &result)?;
        let next_current_namespaces = self
            .current_namespaces
            .map(|view| view.advance(self.state, &next).map(std::sync::Arc::new))
            .transpose()?;
        if let Some(view) = self.current_namespaces {
            // The State proof above cannot constrain Prepared. Bind every copy
            // before the first existing-body probe or any payload preparation.
            view.verify_backup_prepared(self.state, &next, &result, &prepared)?;
            view.verify_history_copy_prepared(self.state, &next, &result, &prepared)?;
        }
        let metadata_epoch = self.metadata_integrity.capture();
        let members = self.members();
        let mut required = BTreeMap::<String, u64>::new();
        // One held descriptor per proof; release all before decision proofs.
        let proof_budget = payload_proof_budget();
        let mut existing = Vec::with_capacity(prepared.len());
        for p in &prepared {
            let mut copies = Vec::with_capacity(p.placement.route_slot_ids.len());
            for slot in &p.placement.route_slot_ids {
                let disk = self.disk(slot)?;
                let directory =
                    version_dir(disk, &p.version.history_tree_id, &p.version.version_id);
                let handle = self.disk_resources.get(disk)?;
                let reservation = proof_budget.reserve(1);
                match VerifiedExisting::read_at(&handle.root, &directory, &p.version) {
                    Ok(proof) => {
                        if let Some(reservation) = reservation {
                            copies.push(Some(RetainedExisting { proof, reservation }));
                        } else {
                            // Large requests retain the bounded-memory reread path.
                            copies.push(None);
                        }
                        continue;
                    }
                    Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                    | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        copies.push(None);
                    }
                    // Existing corrupt/unreadable bodies were never repairable by
                    // preparation. Preserve that cause instead of reporting NoSpace.
                    Err(error) => return Err(error),
                }
                *required.entry(slot.clone()).or_default() = required
                    .get(slot)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(p.version.logical_size)
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::ResourceLimit,
                            "admission bytes overflow",
                        )
                    })?;
            }
            existing.push(std::sync::Mutex::new(Some(copies)));
        }
        for (slot, bytes) in &required {
            let d = self.disk(slot)?;
            let free = self.disk_resources.get(d)?.root.available()?;
            let budget = d
                .capacity_bytes
                .map(|limit| limit.saturating_sub(self.used_bytes(slot)))
                .unwrap_or(u64::MAX);
            if bytes.saturating_add(self.config.metadata_reserve_bytes) > free || *bytes > budget {
                return Err(Error::new(
                    storage_contract::ErrorCode::NoSpace,
                    format!("fixed route slot {slot} cannot reserve the complete payload"),
                ));
            }
        }
        // Resolve real page hashes and enforce every metadata bound without
        // publishing files or journal records. No estimate enters the decision.
        let config_hash = object_hash("publication-config", &self.config)?;
        let baseline = self
            .published_state_base
            .as_ref()
            .filter(|base| base.matches(self, &config_hash));
        let reused_dependencies = baseline.and_then(|base| {
            self.capture_scrub_coverage()
                .map(|coverage| PublishedDependencyReuse {
                    base: std::sync::Arc::clone(base),
                    coverage,
                })
        });
        let mut snapshot = match baseline {
            Some(base) => {
                assert!(
                    base.snapshot_basis.matches_snapshot(&base.snapshot),
                    "invariant: published snapshot descriptor binds the successful publication root"
                );
                prepare_snapshot_incremental(self.state, &next, &base.snapshot_basis, maximum)?
            }
            None => prepare_snapshot(&next, maximum)?,
        };
        snapshot.metadata_integrity = Some(std::sync::Arc::clone(self.metadata_integrity));
        let snapshot_hash = snapshot.hash.clone();
        #[cfg(test)]
        assert_eq!(
            snapshot_hash,
            prepare_snapshot(&next, maximum)?.hash,
            "incremental snapshot differs from full encoding"
        );
        // Snapshot work does not extend an earlier metadata epoch's authority.
        let history_baseline = self
            .published_state_base
            .as_ref()
            .filter(|base| base.matches(self, &config_hash));
        // Membership is derived from immutable State, not disk graph authority.
        let owners = self
            .published_state_base
            .as_ref()
            .filter(|base| base.names_captured_for(self.state))
            .map(|base| &base.history_owners);
        let mut projection =
            crate::state_projection::ProjectionTransition::new(self.state, &next, owners)?;
        let (history_basis, manifests) = if let Some(reuse) = &reused_dependencies {
            crate::history_package::prepare_history_manifests_reusing_published_at(
                self.disk_resources,
                &mut projection,
                reuse,
                &members,
                maximum,
                &mut |error| self.metadata_integrity.fault(error),
            )?
        } else {
            match history_baseline {
                Some(base) => {
                    crate::history_package::prepare_history_manifests_incremental_observed_at(
                        self.disk_resources,
                        &mut projection,
                        &base.roots,
                        &members,
                        maximum,
                        &mut |error| self.metadata_integrity.fault(error),
                    )?
                }
                None => crate::history_package::prepare_history_manifests(&next, maximum)?,
            }
        };
        // The full implementation remains an independent oracle in every
        // engine publication test, including merge/split/restore workflows.
        #[cfg(test)]
        assert_eq!(
            history_basis.reference(),
            crate::history_package::prepare_history_manifests(&next, maximum)?
                .0
                .reference(),
            "incremental history roots differ from complete projection",
        );
        let candidate_data = CandidateData {
            pool_id: self.config.pool_id.clone(),
            sequence: next.sequence,
            transaction_id: match self
                .config
                .generation()
                .and_then(|generation| generation.transaction_id(result.operation_id))
            {
                Some(id) => id,
                None => new_id("tx")?,
            },
            operation_id: result.operation_id,
            request_digest: result.request_digest.clone(),
            previous_snapshot: self.snapshot.clone(),
            snapshot_hash,
            versions: prepared
                .iter()
                .map(|p| VersionEvidence {
                    version: p.version.clone(),
                    placement: p.placement.clone(),
                })
                .collect(),
            metadata_disk_ids: members.iter().map(|d| d.disk_id.clone()).collect(),
            history_roots: history_basis.reference().clone(),
        };
        let candidate = match self
            .config
            .generation()
            .and_then(|generation| generation.registry())
        {
            Some(registry) => Candidate::new_with_registry(
                candidate_data,
                self.config
                    .generation_hash()
                    .expect("invariant: registry belongs to an admitted exit generation"),
                registry,
            ),
            None => Candidate::new(candidate_data, self.config.generation_hash()),
        };
        crate::metadata_budget::preflight_publication_prepared(
            self,
            &candidate,
            &result,
            &next,
            None,
            maximum,
            Some((&snapshot, &history_basis)),
        )?;
        crate::container_storage::validate_new_projection_paths(&projection)?;
        crate::protection::preflight_backup_directories(self, &result)?;
        let preparation = PayloadPreparation::new(
            self.config,
            self.disk_resources,
            &candidate,
            std::sync::Arc::clone(self.metadata_integrity),
        )?;
        self.prepare_payloads(&prepared, existing)?;
        let verified = preparation.verify(self.config, &candidate)?;
        // Payload installation establishes the physical container, but no decision
        // exists yet. Encode its complete projection before ACCEPT can be written.
        let names = crate::container_storage::prepare_names(
            self.config,
            self.disk_resources,
            &projection,
            cache_maximum,
        )?;
        if let Some(reuse) = &reused_dependencies {
            if !reuse.matches(self, &config_hash) {
                return Err(Error::new(
                    ErrorCode::PreparationStale,
                    "published dependency authority changed during preparation",
                )
                .retryable());
            }
            self.metadata_integrity
                .observe(snapshot.store_new_objects_at(self.disk_resources, &members))?;
        } else {
            self.metadata_integrity.observe(snapshot.store_at(
                self.disk_resources,
                &members,
                &next,
            ))?;
        }
        let snapshot_basis = snapshot.reuse_basis();
        for manifest in manifests {
            self.metadata_integrity
                .observe(manifest.store_at(self.disk_resources, &members))?;
        }
        if let Some(reuse) = &reused_dependencies
            && !reuse.matches(self, &config_hash)
        {
            return Err(Error::new(
                ErrorCode::PreparationStale,
                "published dependency authority changed during installation",
            )
            .retryable());
        }
        let history_owners = projection.into_owners();
        Ok(DetachedPublication {
            next,
            next_current_namespaces,
            candidate,
            result,
            verified,
            names,
            metadata_epoch,
            prepared_epoch: self.metadata_integrity.capture(),
            reused_dependencies,
            base_config: self.config.clone(),
            disk_resources: std::sync::Arc::clone(self.disk_resources),
            base_root: self.root.to_path_buf(),
            base_snapshot: self.snapshot.clone(),
            base_sequence: self.state.sequence,
            base_next_slot: self.next_slot,
            config_hash,
            snapshot_basis,
            history_basis,
            history_owners,
        })
    }
}

impl crate::engine::PlanningContext<'_> {
    fn prepare_payloads<'a>(
        &self,
        prepared: &'a [Prepared],
        existing: Vec<std::sync::Mutex<Option<ExistingCopies<'a>>>>,
    ) -> Result<()> {
        // Versions of the same file can share a named container. Preserve their
        // order, and each version's route chain, while independent files copy
        // with bounded parallel workers. No decision is written until all join.
        assert_eq!(
            prepared.len(),
            existing.len(),
            "invariant: each prepared version has its admission results"
        );
        let jobs: Vec<_> = prepared.iter().zip(existing).collect();
        let mut by_file = BTreeMap::new();
        for job in &jobs {
            by_file
                .entry(job.0.version.file_id.as_str())
                .or_insert_with(Vec::new)
                .push(job);
        }
        let groups: Vec<_> = by_file.into_values().collect();
        run_payload_groups(&groups, |(payload, copies)| {
            let copies = copies
                .lock()
                .expect("invariant: payload admission ownership poisoned")
                .take()
                .expect("invariant: payload group is claimed once");
            self.prepare_payload_with_existing(payload, Some(copies))
        })
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn prepare_payload(&self, p: &Prepared) -> Result<()> {
        self.prepare_payload_with_existing(p, None)
    }

    fn prepare_payload_with_existing<'a>(
        &self,
        p: &'a Prepared,
        mut existing: Option<ExistingCopies<'a>>,
    ) -> Result<()> {
        if let Some(copies) = &existing {
            assert_eq!(
                copies.len(),
                p.placement.route_slot_ids.len(),
                "invariant: each routed replica has an admission result"
            );
        }
        let mut source = None;
        for (index, slot) in p.placement.route_slot_ids.iter().enumerate() {
            let disk = self.disk(slot)?;
            let handle = self.disk_resources.get(disk)?;
            let root = &handle.root;
            let directory = version_dir(disk, &p.version.history_tree_id, &p.version.version_id);
            // Keep the lease outside the consuming install call, including its
            // error/unwind paths. That call drops the body FD before returning.
            let mut reservation = None;
            let content = match root
                .read_json_observed::<FileVersion>(&directory.join("version.json"), &mut |_| Ok(()))
            {
                Ok(recorded) => {
                    if object_hash("version", &recorded)? != object_hash("version", &p.version)? {
                        return Err(Error::new(
                            storage_contract::ErrorCode::ObjectCollision,
                            "version identity already exists with different bytes",
                        ));
                    }
                    let verified = match existing.as_mut().and_then(|copies| copies[index].take()) {
                        Some(retained) => {
                            reservation = Some(retained.reservation);
                            retained.proof
                        }
                        None => VerifiedExisting::read_at(root, &directory, &p.version)?,
                    };
                    verified.assert_binding(&directory, &p.version);
                    crate::container_storage::PreparedContent::Verified(verified)
                }
                Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    let staging = disk
                        .root
                        .join("staging")
                        .join(OperationId::generate()?.to_string());
                    root.private_dir(&staging.join("content"))?;
                    let target = staging.join("content").join(&p.version.creation_name);
                    let copied = verified_full_copy_at(
                        root,
                        match source.take() {
                            Some(file) => file,
                            None => p.source.open()?,
                        },
                        &target,
                        &p.version,
                        self.config.max_file_bytes,
                    )
                    .map_err(CopyFailure::into_error)?;
                    root.write_json(&staging.join("version.json"), &p.version, false)?;
                    root.write_atomic(
                        &staging.join("checksums.bin"),
                        &copied.checksums().encode()?,
                        false,
                    )?;
                    root.write_json(
                        &staging.join("payload.json"),
                        &json!({"payload_id":format!("payload_{}",&p.version.version_id[4..]),"version_id":p.version.version_id,"kind":"FULL","size":p.version.logical_size.to_string(),"content_hash":p.version.content_hash}),
                        false,
                    )?;
                    root.sync_dir(&staging)?;
                    root.private_dir(directory.parent().expect("version parent"))?;
                    // The formal payload is never cleanup scratch after rename.
                    // Its directory barriers finish under the same admitted owner.
                    root.rename_noreplace_destination_first_synced(&staging, &directory)?;
                    crate::container_storage::PreparedContent::Copied(copied)
                }
                Err(error) => return Err(error),
            };
            // Install a real version file in this file's named container before
            // any protection receipt is issued. The next level reads this copy.
            let path =
                crate::container_storage::install_version_content_at(root, &directory, content)?;
            drop(reservation);
            if index + 1 < p.placement.route_slot_ids.len() {
                source = Some(ReadFile::from(root.open_leased_regular(&path)?));
            }
        }
        Ok(())
    }
}
