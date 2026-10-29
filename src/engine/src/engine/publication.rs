//! Durable preparation, decision and publication of one frozen transaction.
use super::{Engine, Prepared};
use crate::journal::RecordType;
use crate::{disk::*, journal::*, *};
use json_codec::json;
use std::collections::BTreeMap;
use std::sync::Arc;
#[path = "verified_payloads.rs"]
mod verified_payloads;
use verified_payloads::{MAX_HELD_PAYLOAD_FILES, PayloadPreparation, VerifiedPayloadSet};
#[path = "publication/stages.rs"]
mod stages;
use stages::TaskOutcome;
pub use stages::{PublicationStageReport, StageIo, StageKind, StageTiming};
#[path = "publication/member_protocol.rs"]
mod member_protocol;
#[cfg(feature = "fault-injection")]
pub(super) use member_protocol::ArmedMemberFault;
#[cfg(feature = "fault-injection")]
pub use member_protocol::MemberFault;
pub(crate) use member_protocol::StageSequence;
use member_protocol::{AppendMode, ProofDisks, group_proof_targets, target_items};

type ExistingCopies<'a> = Vec<Option<VerifiedExisting<'a>>>;

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

    /// The owner index captured with this publication; valid only for callers
    /// that checked `names_captured_for` against the current State allocation.
    pub(super) fn history_owners(&self) -> &crate::state_projection::DerivedOwners {
        &self.history_owners
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
    /// Preparation stage timings, prepended to the decision stages.
    preparation_stages: Vec<StageTiming>,
    preparation_started: std::time::Instant,
}

enum Completion<'a> {
    Prepared {
        payloads: &'a VerifiedPayloadSet,
        names: crate::container_storage::PreparedNames,
    },
    Recovery,
}

/// Logical bytes of a successful proof install plus explicitly observed reads.
/// Compare-and-sync of an identical proof counts as a completed install.
fn proof_io(written: &[u8], read: u64) -> StageIo {
    StageIo {
        proofs_completed: 1,
        logical_bytes_completed: written.len() as u64,
        observed_bytes_read: read,
        ..StageIo::default()
    }
}

impl Engine {
    #[cfg(test)]
    pub(crate) fn persist(
        &mut self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
    ) -> Result<()> {
        self.persist_with_objects(next, prepared, Vec::new(), result)
    }
    pub(crate) fn persist_with_objects(
        &mut self,
        next: State,
        prepared: Vec<Prepared>,
        namespace: Vec<crate::journal::PreparedMetadataObject>,
        result: OperationResult,
    ) -> Result<()> {
        self.persist_with_metadata_limit(next, prepared, namespace, result, MAX_METADATA)
    }
    pub(crate) fn persist_with_metadata_limit(
        &mut self,
        next: State,
        prepared: Vec<Prepared>,
        namespace: Vec<crate::journal::PreparedMetadataObject>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<()> {
        let detached = self.prepare_publication(next, prepared, namespace, result, maximum)?;
        self.decide_publication(detached).map(|_| ())
    }
    pub(crate) fn prepare_publication(
        &self,
        next: State,
        prepared: Vec<Prepared>,
        namespace: Vec<crate::journal::PreparedMetadataObject>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<DetachedPublication> {
        self.planning_context()
            .prepare_publication(next, prepared, namespace, result, maximum)
    }
    pub(crate) fn decide_publication(
        &mut self,
        detached: DetachedPublication,
    ) -> Result<OperationResult> {
        if self.blocked
            || !detached.prepared_epoch.matches(&self.metadata_integrity)
            || detached.reused_dependencies.as_ref().is_some_and(|reuse| {
                !reuse.matches(&self.planning_context(), &detached.config_hash)
            })
            || !self.config.same_admission(&detached.base_config)
            || !Arc::ptr_eq(&self.disk_resources, &detached.disk_resources)
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
        let DetachedPublication {
            next,
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
            preparation_stages,
            preparation_started,
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
        let mut stages = StageSequence::started_at(&hash, members.len(), preparation_started);
        stages.prepend(preparation_stages);
        // PREPARE: candidate object store then PREPARE append, per member.
        // A failure here is reported as is; no ACCEPT exists anywhere yet.
        let prepare = self.run_member_stage(
            &mut stages,
            StageKind::Prepare,
            &members,
            Some("after_prepare_"),
            |task| {
                let mut io = StageIo::default();
                let value = (|| {
                    candidate_object.store_on(&task.handle.root, task.disk)?;
                    io.objects_completed += 1;
                    io.logical_bytes_completed += candidate_object.bytes().len() as u64;
                    member_protocol::append_record(
                        task,
                        AppendMode::Live,
                        RecordType::Prepare,
                        candidate.sequence,
                        &hash,
                        &candidate.snapshot_hash,
                        &mut io,
                    )
                })();
                TaskOutcome { value, io }
            },
        );
        if let Err(error) = prepare {
            self.last_publication_stages = Some(stages.finish("prepare-failed"));
            return Err(error);
        }
        drop(candidate_object);
        let admission = (|| {
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
            Ok(())
        })();
        if let Err(error) = admission {
            self.last_publication_stages = Some(stages.finish("admission-refused"));
            return Err(error);
        }
        // Admission is after every PREPARE and before first ACCEPT. The short
        // schedule/epoch fence has now committed to starting this decision;
        // stop/fault after it cannot cancel publication or release ownership.
        self.blocked = true;
        let finish = (|| {
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_decision_admission")?;
            self.complete_candidate_verified(
                &candidate,
                &hash,
                &next,
                None,
                Completion::Prepared {
                    payloads: &verified,
                    names,
                },
                &mut stages,
            )
        })();
        if let Err(e) = finish {
            self.metadata_integrity.invalidate();
            let message = match stages.failure() {
                Some(stage) => format!(
                    "{e} ({stage}); query operation {} and recover",
                    result.operation_id
                ),
                None => format!("{e}; query operation {} and recover", result.operation_id),
            };
            self.last_publication_stages = Some(stages.finish("in-doubt"));
            return Err(e
                .context(storage_contract::ErrorCode::InDoubt, message)
                .retryable());
        }
        // The published basis now carries the receipt log's encoding lineage;
        // release the one-shot cold index and record the locator root for
        // on-demand id lookups.
        let mut next = next;
        next.operations.release_cold_index();
        self.operation_locator_root = Some(
            snapshot_basis
                .descriptor
                .maps
                .get("operation_locators")
                .cloned()
                .expect("invariant: the validated snapshot schema carries its locator root"),
        );
        let previous = std::mem::replace(&mut self.state, std::sync::Arc::new(next));
        self.snapshot = Some(candidate.snapshot_hash.clone());
        self.update_index(&previous);
        // The published catalog is this State's root identity for history reads.
        // It is bound unconditionally: a root names admitted content, it is not
        // the metadata-epoch reuse permit tracked by the base below.
        self.history_catalog = Some(super::history_window::HistoryCatalog::published(
            &self.state,
            history_basis.clone(),
        ));
        // Only the completed member publication establishes this proof. SQLite is
        // a disposable accelerator, not a dependency of the authoritative closure.
        self.published_state_base = metadata_epoch.matches(&self.metadata_integrity).then(|| {
            Arc::new(PublishedStateBase {
                state: Arc::downgrade(&self.state),
                metadata_epoch,
                snapshot: candidate.snapshot_hash,
                sequence: self.state.sequence,
                config_hash,
                config: self.config.clone(),
                roots: history_basis,
                snapshot_basis,
                history_owners,
            })
        });
        self.last_publication_stages = Some(stages.finish("published"));
        self.blocked = false;
        Ok(result)
    }
    pub(crate) fn prepare_payload(&self, p: &Prepared) -> Result<()> {
        self.planning_context().prepare_payload(p)
    }

    pub(crate) fn complete_candidate(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        existing: Option<Publication>,
    ) -> Result<()> {
        let mut stages = StageSequence::new(hash, self.members().len());
        let result = self.complete_candidate_verified(
            c,
            hash,
            next,
            existing,
            Completion::Recovery,
            &mut stages,
        );
        self.last_publication_stages = Some(stages.finish(if result.is_ok() {
            "recovered"
        } else {
            "recovery-failed"
        }));
        result
    }
    /// Replay a verified existing Publication onto every member. `receipt`
    /// resolves one member's PUBLICATION receipt: an already journaled frame is
    /// returned as is, otherwise the record is appended in `mode`.
    pub(super) fn complete_published_candidate(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        publication: Publication,
        existing_receipt: impl Fn(&DiskConfig, &str) -> Option<Receipt> + Sync,
        mode: AppendMode,
        stages: &mut StageSequence,
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
        crate::container_storage::repair_historical_names(
            &self.config,
            &self.disk_resources,
            &self.state,
            next,
        )?;
        let publication_object =
            PreparedMetadataObject::new("publication", &publication, MAX_METADATA)?;
        let pub_hash = publication_object.hash.clone();
        let members = self.members();
        let publication_receipts = self.run_member_stage(
            stages,
            StageKind::Publication,
            &members,
            // Only a recovery publication has per-member failpoints on replay.
            publication
                .recovery
                .is_some()
                .then_some("after_recovery_publication_"),
            |task| {
                let mut io = StageIo::default();
                let value = (|| {
                    publication_object.store_on(&task.handle.root, task.disk)?;
                    io.objects_completed += 1;
                    io.logical_bytes_completed += publication_object.bytes().len() as u64;
                    match existing_receipt(task.disk, &pub_hash) {
                        Some(receipt) => Ok(receipt),
                        None => member_protocol::append_record(
                            task,
                            mode,
                            RecordType::Publication,
                            c.sequence,
                            hash,
                            &pub_hash,
                            &mut io,
                        ),
                    }
                })();
                TaskOutcome { value, io }
            },
        )?;
        drop(publication_object);
        let seal = PublicationSeal {
            candidate_hash: hash.into(),
            publication_hash: pub_hash,
            receipts: publication_receipts.clone(),
        };
        verify_publication_seal(c, &publication, &seal)?;
        self.write_seals(stages, &members, hash, &seal)?;
        // Historical success is preserved even if bytes were later purged or lost.
        // Reconstruct only its original proof, never claim a newly verified payload.
        // A payload disk absent from this admission is skipped, as before.
        let armed = self.take_member_fault(StageKind::PublicationProofs);
        let targets = group_proof_targets(self, c, ProofDisks::Available)?;
        let state = &self.state;
        self.run_disk_stage(
            stages,
            StageKind::PublicationProofs,
            &armed,
            &targets,
            |task| {
                let mut io = StageIo::default();
                let value = (|| {
                    for &(index, slot) in target_items(&targets, task) {
                        let evidence = &c.versions[index];
                        repair_historical_proofs(
                            state,
                            task.handle,
                            task.disk,
                            slot,
                            evidence,
                            hash,
                            &publication,
                            &publication_receipts,
                            c,
                            &mut io,
                        )?;
                    }
                    Ok(())
                })();
                TaskOutcome { value, io }
            },
        )?;
        Ok(())
    }
    fn complete_candidate_verified(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        existing: Option<Publication>,
        completion: Completion<'_>,
        stages: &mut StageSequence,
    ) -> Result<()> {
        let (verified, names, mode) = match completion {
            Completion::Prepared { payloads, names } => {
                (Some(payloads), Some(names), AppendMode::Live)
            }
            Completion::Recovery => (None, None, AppendMode::Recovery),
        };
        if let Some(publication) = existing {
            return self.complete_published_candidate(
                c,
                hash,
                next,
                publication,
                |_, _| None,
                mode,
                stages,
            );
        }
        self.verify_candidate_membership(c)?;
        let names = match names {
            Some(names) => names,
            None => {
                let projection_members = self.members();
                let published = crate::namespace::resources_source(
                    &self.disk_resources,
                    &projection_members,
                    &self.segment_caches,
                );
                let projection = crate::state_projection::ProjectionTransition::new(
                    &self.state,
                    next,
                    None,
                    &published,
                )?;
                crate::container_storage::prepare_names(
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
        // ACCEPT on every member; the decision stands once Q members have
        // synced, but no later stage starts before all members returned.
        let accepts = self.run_member_stage(
            stages,
            StageKind::Accept,
            &members,
            Some("after_accept_"),
            |task| {
                let mut io = StageIo::default();
                let value = member_protocol::append_record(
                    task,
                    mode,
                    RecordType::Accept,
                    c.sequence,
                    hash,
                    hash,
                    &mut io,
                );
                TaskOutcome { value, io }
            },
        )?;
        let finalize = Finalize {
            candidate_hash: hash.into(),
            accepts,
            recovery_id: None,
        };
        let finalize_object = PreparedMetadataObject::new("finalize", &finalize, MAX_METADATA)?;
        let final_hash = finalize_object.hash.clone();
        let final_receipts =
            self.run_member_stage(stages, StageKind::Finalize, &members, None, |task| {
                let mut io = StageIo::default();
                let value = (|| {
                    finalize_object.store_on(&task.handle.root, task.disk)?;
                    io.objects_completed += 1;
                    io.logical_bytes_completed += finalize_object.bytes().len() as u64;
                    member_protocol::append_record(
                        task,
                        mode,
                        RecordType::Finalize,
                        c.sequence,
                        hash,
                        &final_hash,
                        &mut io,
                    )
                })();
                TaskOutcome { value, io }
            })?;
        drop(finalize_object);
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_finalize")?;
        // Only a decided candidate may rename a published file container.
        // A crash leaves the operation indeterminate and recovery replays it.
        // Names share one temporary cursor and stay serial.
        let began = std::time::Instant::now();
        names.apply()?;
        stages.record_serial(StageKind::Names, began, members.len(), StageIo::default());
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_container_names")?;
        // Commit proofs: one task per payload disk, (version, slot) pairs in
        // candidate order within the disk; the receipt list is sorted below.
        let armed = self.take_member_fault(StageKind::CommitProofs);
        let targets = group_proof_targets(self, c, ProofDisks::Required)?;
        let config = &self.config;
        let receipts =
            self.run_disk_stage(stages, StageKind::CommitProofs, &armed, &targets, |task| {
                let items = target_items(&targets, task);
                let mut io = StageIo::default();
                let mut receipts = Vec::with_capacity(items.len());
                let value = (|| {
                    for &(index, slot) in items {
                        let evidence = &c.versions[index];
                        let d = task.disk;
                        let handle = task.handle;
                        let directory = version_dir(
                            d,
                            &evidence.version.history_tree_id,
                            &evidence.version.version_id,
                        );
                        if let Some(verified) = verified {
                            verified.validate_payload(config, &evidence.version, slot)?;
                        } else {
                            // Recovery has no live held-inode proof and must re-read all bytes.
                            handle.verify_payload(&directory, &evidence.version)?;
                            io.observed_bytes_read += evidence.version.logical_size;
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
                        // One encoding gives the written bytes and the receipt hash.
                        let proof =
                            PreparedMetadataObject::new("commit-proof", &proof, MAX_METADATA)?;
                        handle.root.write_atomic(
                            &proof_dir.join("commit-proof.json"),
                            proof.bytes(),
                            false,
                        )?;
                        io.add(proof_io(proof.bytes(), 0));
                        receipts.push(PayloadReceipt {
                            version_id: evidence.version.version_id.clone(),
                            disk_id: d.disk_id.clone(),
                            slot_id: d.slot_id.clone(),
                            proof_hash: proof.hash,
                        });
                    }
                    Ok(receipts)
                })();
                TaskOutcome { value, io }
            })?;
        let mut payload_receipts: Vec<PayloadReceipt> = receipts.into_iter().flatten().collect();
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
            result: self
                .candidate_receipt(next, &members, c.operation_id)?
                .map(|entry| entry.result)
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
        let publication_receipts = self.run_member_stage(
            stages,
            StageKind::Publication,
            &members,
            Some("after_publication_"),
            |task| {
                let mut io = StageIo::default();
                let value = (|| {
                    publication_object.store_on(&task.handle.root, task.disk)?;
                    io.objects_completed += 1;
                    io.logical_bytes_completed += publication_object.bytes().len() as u64;
                    member_protocol::append_record(
                        task,
                        mode,
                        RecordType::Publication,
                        c.sequence,
                        hash,
                        &pub_hash,
                        &mut io,
                    )
                })();
                TaskOutcome { value, io }
            },
        )?;
        drop(publication_object);
        let seal = PublicationSeal {
            candidate_hash: hash.into(),
            publication_hash: pub_hash,
            receipts: publication_receipts.clone(),
        };
        self.write_seals(stages, &members, hash, &seal)?;
        let armed = self.take_member_fault(StageKind::PublicationProofs);
        let targets = group_proof_targets(self, c, ProofDisks::Required)?;
        self.run_disk_stage(
            stages,
            StageKind::PublicationProofs,
            &armed,
            &targets,
            |task| {
                let mut io = StageIo::default();
                let value = (|| {
                    for &(index, _) in target_items(&targets, task) {
                        let evidence = &c.versions[index];
                        let directory = version_dir(
                            task.disk,
                            &evidence.version.history_tree_id,
                            &evidence.version.version_id,
                        );
                        let root = &task.handle.root;
                        let proof_dir = local_proof_directory_at(root, &directory, hash)?;
                        let mut read = 0u64;
                        let proof: CommitProof = root.read_json_observed(
                            &proof_dir.join("commit-proof.json"),
                            &mut |count| {
                                read += count as u64;
                                Ok(())
                            },
                        )?;
                        let bytes = encode_metadata(
                            &LocalPublicationRef {
                                candidate: c,
                                publication: &publication,
                                commit_proof: &proof,
                                publication_receipts: &publication_receipts,
                            },
                            MAX_METADATA,
                        )?;
                        root.write_atomic(
                            &proof_dir.join("publication-proof.json"),
                            &bytes,
                            false,
                        )?;
                        io.add(proof_io(&bytes, read));
                    }
                    Ok(())
                })();
                TaskOutcome { value, io }
            },
        )?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_local_publications")?;
        Ok(())
    }

    /// PublicationSeal on every member, one task per member, no journal write.
    fn write_seals(
        &mut self,
        stages: &mut StageSequence,
        members: &[DiskConfig],
        hash: &str,
        seal: &PublicationSeal,
    ) -> Result<()> {
        let bytes = encode_metadata(seal, MAX_METADATA)?;
        self.run_member_stage(stages, StageKind::Seals, members, None, |task| {
            let value = task.handle.root.write_atomic(
                &publication_seal_path(task.disk, hash),
                &bytes,
                false,
            );
            let io = match value {
                Ok(()) => proof_io(&bytes, 0),
                Err(_) => StageIo::default(),
            };
            TaskOutcome { value, io }
        })?;
        Ok(())
    }
}

/// Reconstruct one slot's historical commit and publication proofs from the
/// verified publication closure, without claiming a newly verified payload.
#[expect(
    clippy::too_many_arguments,
    reason = "one proof reconstruction needs the whole verified closure"
)]
fn repair_historical_proofs(
    state: &State,
    handle: &DiskHandle,
    d: &DiskConfig,
    slot: &str,
    evidence: &VersionEvidence,
    hash: &str,
    publication: &Publication,
    publication_receipts: &[Receipt],
    c: &Candidate,
    io: &mut StageIo,
) -> Result<()> {
    let dir = version_dir(
        d,
        &evidence.version.history_tree_id,
        &evidence.version.version_id,
    );
    match handle.root.directory(&dir) {
        Ok(_) => {}
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
        | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    }
    let proof = CommitProof {
        candidate_hash: hash.into(),
        finalize: publication.finalize.clone(),
        version_hash: object_hash("version", &evidence.version)?,
        disk_id: d.disk_id.clone(),
        slot_id: slot.to_owned(),
        route_slot_ids: evidence.placement.route_slot_ids.clone(),
    };
    let expected = publication
        .payload_receipts
        .iter()
        .find(|r| r.version_id == evidence.version.version_id && r.slot_id == slot);
    let Some(expected) = expected else {
        if publication.recovery.is_some() {
            return Ok(());
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
    let first_on_slot = state
        .placements
        .get(&evidence.version.version_id)
        .is_none_or(|placement| !placement.route_slot_ids.contains(&slot.to_owned()));
    let proof_dir = if first_on_slot {
        dir
    } else {
        local_proof_directory_at(&handle.root, &dir, hash)?
    };
    io.add(repair_local_proof(
        &handle.root,
        &proof_dir.join("commit-proof.json"),
        &proof,
    )?);
    io.add(repair_local_proof(
        &handle.root,
        &proof_dir.join("publication-proof.json"),
        &LocalPublicationRef {
            candidate: c,
            publication,
            commit_proof: &proof,
            publication_receipts,
        },
    )?);
    Ok(())
}

/// Restore only a proof reconstructed from a verified published closure.
/// Conflicting local material is retained under the same version directory;
/// recovery never deletes it or treats it as authority for a different result.
fn repair_local_proof<T: data_serialization::Serialize>(
    root: &DiskRoot,
    path: &std::path::Path,
    value: &T,
) -> Result<StageIo> {
    use platform_io::fs_safety::linux::FileIdentity;
    let expected = encode_metadata(value, MAX_METADATA)?;
    let original = match root.open_regular(path) {
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
        | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            root.write_atomic(path, &expected, false)?;
            return Ok(proof_io(&expected, 0));
        }
        Err(error) => return Err(error),
        Ok(file) => file,
    };
    // Fail closed on links/special files and I/O errors. An oversized proof can
    // be retained by rename without allocating or interpreting its whole body.
    let mut read = 0u64;
    match root.read_bytes(path, MAX_METADATA) {
        Ok(bytes) if bytes == expected => {
            return Ok(StageIo {
                observed_bytes_read: bytes.len() as u64,
                ..StageIo::default()
            });
        }
        Ok(bytes) => read = bytes.len() as u64,
        Err(error) if error.code() == storage_contract::ErrorCode::ResourceLimit => {}
        Err(error) => return Err(error),
    }
    let parent = path.parent().expect("proof parent");
    let quarantine = parent.join("proof-quarantine");
    root.private_dir(&quarantine)?;
    let source = root.directory(parent)?;
    let destination = root.directory(&quarantine)?;
    if FileIdentity::from_fd(&original)? != FileIdentity::from_fd(root.open_regular(path)?)? {
        return Err(Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "local proof identity changed before repair",
        ));
    }
    let name = format!(
        "{}-{}",
        path.file_name().expect("proof name").to_string_lossy(),
        new_id("req")
    );
    rustix::fs::renameat_with(
        &source,
        path.file_name().expect("proof name"),
        &destination,
        name.as_str(),
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)?;
    destination.sync_all()?;
    source.sync_all()?;
    if FileIdentity::from_fd(&original)?
        != FileIdentity::from_fd(root.open_regular(&quarantine.join(name))?)?
    {
        return Err(Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "local proof identity changed during repair",
        ));
    }
    root.write_atomic(path, &expected, false)?;
    Ok(proof_io(&expected, read))
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
                self.0.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
            }
        }
    }
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(WORKERS.min(groups.len()));
        let mut startup_error = None;
        for _ in 0..WORKERS.min(groups.len()) {
            match std::thread::Builder::new().spawn_scoped(scope, || {
                loop {
                    let index = {
                        let mut queue = queue
                            .lock()
                            .expect("invariant: payload preparation queue poisoned");
                        if queue.closed || queue.first_error.is_some() || queue.next == groups.len()
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
            }) {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    queue
                        .lock()
                        .expect("invariant: payload preparation queue poisoned")
                        .closed = true;
                    startup_error = Some(Error::from(error));
                    break;
                }
            }
        }
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
        if let Some(error) = startup_error {
            let mut queue = queue
                .lock()
                .expect("invariant: payload preparation queue poisoned");
            if queue.first_error.is_none() {
                queue.first_error = Some((usize::MAX, error));
            }
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

#[cfg(all(test, feature = "fault-injection"))]
#[path = "../../../../test/engine/unit/engine/publication/member_fault_tests.rs"]
mod member_fault_tests;

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn prepare_publication(
        &self,
        next: State,
        prepared: Vec<Prepared>,
        namespace: Vec<crate::journal::PreparedMetadataObject>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<DetachedPublication> {
        self.prepare_publication_bounded(next, prepared, namespace, result, maximum, maximum)
    }
}

impl crate::engine::PlanningContext<'_> {
    /// The complete id→ordinal locator index rebuilt through the published
    /// locator root plus this transaction's resident appends. A full snapshot
    /// encoding needs it after the one-shot cold index was consumed: a durable
    /// local preparation (a saved plan, a historical-save bundle) edits the
    /// visible state outside publication, which deliberately drops the
    /// incremental baseline so the next snapshot captures that edit in full.
    fn rebuilt_cold_locator_index(
        &self,
        operations: &storage_contract::OperationLog,
    ) -> Result<std::collections::BTreeMap<storage_contract::OperationId, u64>> {
        let members = self.members();
        let lineage = || {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "operation locator lineage is unavailable; recover before publishing",
            )
        };
        // Before the first decision of this process the root is not resident;
        // the admitted published snapshot carries it.
        let admitted;
        let root = match self.operation_locator_root {
            Some(root) => root,
            None => {
                let hash = self.snapshot.as_deref().ok_or_else(lineage)?;
                let snapshot: Snapshot = crate::publication_evidence::verified_object_at(
                    self.disk_resources,
                    &members,
                    "snapshot",
                    hash,
                )?;
                admitted = snapshot
                    .maps
                    .get("operation_locators")
                    .cloned()
                    .ok_or_else(lineage)?;
                &admitted
            }
        };
        fn member_loader<'a>(
            resources: &'a crate::disk::DiskResources,
            members: &'a [DiskConfig],
        ) -> impl FnMut(&'static str, &str) -> Result<crate::journal::AuthenticatedMetadataObject> + 'a
        {
            move |kind, hash| {
                crate::publication_evidence::verified_graph_object_at(
                    resources, members, kind, hash,
                )
            }
        }
        let page_map =
            crate::metadata_map::read_map(root, &mut member_loader(self.disk_resources, &members))?;
        let mut index = std::collections::BTreeMap::new();
        for (key, value) in page_map {
            let text: String = json_codec::from_value(value)?;
            index.insert(key.parse()?, storage_contract::parse_u64_decimal(&text)?);
        }
        if let Some((_, segment)) = operations.fresh_seal() {
            let first = operations.tail_first_ordinal() - segment.records.len() as u64;
            for (offset, record) in segment.records.iter().enumerate() {
                index.insert(record.result.operation_id, first + offset as u64);
            }
        }
        for (offset, record) in operations.tail().iter().enumerate() {
            index.insert(
                record.result.operation_id,
                operations.tail_first_ordinal() + offset as u64,
            );
        }
        Ok(index)
    }
    /// Test oracle only: a full encoding of `next` compared byte for byte
    /// against the incremental one, rebuilding the locator index when the
    /// resident cold index was already consumed.
    #[cfg(test)]
    pub(super) fn oracle_full_snapshot_hash(&self, next: &State, maximum: usize) -> Result<String> {
        if next.operations.cold_index().is_some() {
            return Ok(crate::journal::prepare_snapshot(next, maximum)?.hash);
        }
        let index = self.rebuilt_cold_locator_index(&next.operations)?;
        let mut oracle = next.clone();
        oracle.operations.attach_cold_index(index)?;
        Ok(crate::journal::prepare_snapshot(&oracle, maximum)?.hash)
    }
    fn prepare_publication_bounded(
        &self,
        mut next: State,
        prepared: Vec<Prepared>,
        namespace: Vec<crate::journal::PreparedMetadataObject>,
        result: OperationResult,
        maximum: usize,
        cache_maximum: usize,
    ) -> Result<DetachedPublication> {
        let metadata_epoch = self.metadata_integrity.capture();
        let members = self.members();
        let mut required = BTreeMap::<String, u64>::new();
        // One held descriptor per proof; release all before decision proofs.
        let mut held = 0;
        let mut existing = Vec::with_capacity(prepared.len());
        for p in &prepared {
            let mut copies = Vec::with_capacity(p.placement.route_slot_ids.len());
            for slot in &p.placement.route_slot_ids {
                let disk = self.disk(slot)?;
                let directory =
                    version_dir(disk, &p.version.history_tree_id, &p.version.version_id);
                let handle = self.disk_resources.get(disk)?;
                match VerifiedExisting::read_at(&handle.root, &directory, &p.version) {
                    Ok(proof) => {
                        if held < MAX_HELD_PAYLOAD_FILES {
                            held += 1;
                            copies.push(Some(proof));
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
                prepare_snapshot_incremental(
                    self.state,
                    &next,
                    &base.snapshot_basis,
                    maximum,
                    &mut |kind, hash| {
                        crate::publication_evidence::verified_graph_object_at(
                            self.disk_resources,
                            &self.members(),
                            kind,
                            hash,
                        )
                    },
                )?
            }
            None => {
                // A full encoding after the one-shot cold index was consumed
                // (a durable local preparation dropped the baseline) rebuilds
                // the locator index through the published root first.
                if next.operations.cold_index().is_none() {
                    let index = self.rebuilt_cold_locator_index(&next.operations)?;
                    next.operations.attach_cold_index(index)?;
                }
                prepare_snapshot(&next, maximum)?
            }
        };
        snapshot.metadata_integrity = Some(std::sync::Arc::clone(self.metadata_integrity));
        let snapshot_hash = snapshot.hash.clone();
        #[cfg(test)]
        assert_eq!(
            snapshot_hash,
            self.oracle_full_snapshot_hash(&next, maximum)?,
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
        let mut projection = {
            let published = crate::namespace::resources_source(
                self.disk_resources,
                &members,
                self.segment_caches,
            );
            // New commits reference namespace objects built by this
            // transaction; resolve them before the published store.
            let source = crate::namespace::PreparedSource::new(&namespace, &published)?;
            crate::state_projection::ProjectionTransition::new(self.state, &next, owners, &source)?
        };
        // The transaction's namespace objects install with the snapshot's own
        // objects, before the decision; the prepared root stays the final
        // element of the object list.
        if !namespace.is_empty() {
            snapshot.objects.splice(0..0, namespace);
        }
        let (history_basis, manifests) = {
            // Fresh namespace objects already ride in the snapshot's object
            // list; the manifest projection resolves them before the store.
            let published = crate::namespace::resources_source(
                self.disk_resources,
                &members,
                self.segment_caches,
            );
            let source = crate::namespace::PreparedSource::new(&snapshot.objects, &published)?;
            if let Some(reuse) = &reused_dependencies {
                crate::history_package::prepare_history_manifests_reusing_published_at(
                    self.disk_resources,
                    &mut projection,
                    reuse,
                    &members,
                    maximum,
                    &mut |error| self.metadata_integrity.fault(error),
                    &source,
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
                            &source,
                        )?
                    }
                    None => {
                        crate::history_package::prepare_history_manifests(&next, maximum, &source)?
                    }
                }
            }
        };
        // The full implementation remains an independent oracle in every
        // engine publication test, including merge/split/restore workflows.
        #[cfg(test)]
        {
            let published = crate::namespace::resources_source(
                self.disk_resources,
                &members,
                self.segment_caches,
            );
            let source = crate::namespace::PreparedSource::new(&snapshot.objects, &published)?;
            let oracle =
                crate::history_package::prepare_history_manifests(&next, maximum, &source)?;
            assert_eq!(
                history_basis.reference(),
                oracle.0.reference(),
                "incremental history roots differ from complete projection",
            );
        }
        let candidate = Candidate {
            pool_id: self.config.pool_id.clone(),
            sequence: next.sequence,
            transaction_id: new_id("tx"),
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
        let preparation = PayloadPreparation::new(
            self.config,
            self.disk_resources,
            &candidate,
            std::sync::Arc::clone(self.metadata_integrity),
        )?;
        let preparation_started = std::time::Instant::now();
        let mut preparation_stages = Vec::new();
        let (payload_io, payload_workers) = self.prepare_payloads(&prepared, existing)?;
        let mut payload_timing = preparation_timing(
            StageKind::PayloadCopy,
            preparation_started,
            preparation_started,
            prepared.len(),
            payload_io,
        );
        payload_timing.workers = payload_workers;
        payload_timing.parallel = payload_workers > 1;
        preparation_stages.push(payload_timing);
        let verified = preparation.verify(self.config, &candidate)?;
        // Payload installation establishes the physical container, but no decision
        // exists yet. Encode its complete projection before ACCEPT can be written.
        let names = crate::container_storage::prepare_names(
            self.config,
            self.disk_resources,
            &projection,
            cache_maximum,
        )?;
        let began = std::time::Instant::now();
        let mut installs = ObjectInstalls::default();
        if let Some(reuse) = &reused_dependencies {
            if !reuse.matches(self, &config_hash) {
                return Err(Error::new(
                    ErrorCode::PreparationStale,
                    "published dependency authority changed during preparation",
                )
                .retryable());
            }
            self.metadata_integrity
                .observe(snapshot.store_new_objects_at(
                    self.disk_resources,
                    &members,
                    &mut installs,
                ))?;
        } else {
            self.metadata_integrity.observe(snapshot.store_at(
                self.disk_resources,
                &members,
                &next,
                &mut installs,
            ))?;
        }
        preparation_stages.push(preparation_timing(
            StageKind::SnapshotInstall,
            preparation_started,
            began,
            members.len(),
            installs_io(installs),
        ));
        let snapshot_basis = snapshot.reuse_basis();
        let began = std::time::Instant::now();
        let mut installs = ObjectInstalls::default();
        for manifest in manifests {
            for disk in &members {
                self.metadata_integrity
                    .observe(manifest.store_on(&self.disk_resources.get(disk)?.root, disk))?;
                installs.record(manifest.bytes().len());
            }
        }
        preparation_stages.push(preparation_timing(
            StageKind::ManifestInstall,
            preparation_started,
            began,
            members.len(),
            installs_io(installs),
        ));
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
            preparation_stages,
            preparation_started,
        })
    }
}

/// Preparation stages run on the calling thread (payload groups fan out
/// inside); the timing records them as serial units of the preparation.
fn preparation_timing(
    stage: StageKind,
    sequence_start: std::time::Instant,
    began: std::time::Instant,
    members_completed: usize,
    io: StageIo,
) -> StageTiming {
    StageTiming {
        stage,
        started_at: began.duration_since(sequence_start),
        elapsed: began.elapsed(),
        parallel: false,
        workers: 1,
        members_completed,
        io,
    }
}

fn installs_io(installs: ObjectInstalls) -> StageIo {
    StageIo {
        objects_completed: installs.objects,
        logical_bytes_completed: installs.bytes,
        ..StageIo::default()
    }
}

impl crate::engine::PlanningContext<'_> {
    fn prepare_payloads<'a>(
        &self,
        prepared: &'a [Prepared],
        existing: Vec<std::sync::Mutex<Option<ExistingCopies<'a>>>>,
    ) -> Result<(StageIo, usize)> {
        // Versions of the same file can share a named container. Preserve their
        // order, and each version's route chain, while independent files copy
        // with bounded parallel workers. No decision is written until all join.
        let jobs: Vec<_> = prepared.iter().zip(existing).collect();
        let mut by_file = BTreeMap::new();
        for job in &jobs {
            by_file
                .entry(job.0.version.file_id.as_str())
                .or_insert_with(Vec::new)
                .push(job);
        }
        let groups: Vec<_> = by_file.into_values().collect();
        let io = std::sync::Mutex::new(StageIo::default());
        let work =
            |(payload, copies): &(&'a Prepared, std::sync::Mutex<Option<ExistingCopies<'a>>>)| {
                let copies = copies
                    .lock()
                    .expect("invariant: payload admission ownership poisoned")
                    .take()
                    .expect("invariant: payload group is claimed once");
                let done = self.prepare_payload_with_existing(payload, copies)?;
                io.lock()
                    .expect("invariant: payload accounting lock poisoned")
                    .add(done);
                Ok(())
            };
        let workers = if prepared.iter().all(|payload| {
            fan_out_admitted(&payload.version, payload.placement.route_slot_ids.len())
        }) {
            run_payload_groups(&groups, work)?;
            groups.len().min(4)
        } else {
            // Preserve large files and routes without multiplying the existing
            // single-copy checksum budget by four concurrent file groups.
            for group in &groups {
                for job in group {
                    work(job)?;
                }
            }
            usize::from(!groups.is_empty())
        };
        Ok((
            io.into_inner()
                .expect("invariant: payload accounting lock poisoned"),
            workers,
        ))
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn prepare_payload(&self, p: &Prepared) -> Result<()> {
        self.prepare_payload_with_existing(p, Vec::new())
            .map(|_| ())
    }

    fn prepare_payload_with_existing<'a>(
        &self,
        p: &'a Prepared,
        mut existing: ExistingCopies<'a>,
    ) -> Result<StageIo> {
        let mut io = StageIo::default();
        // Levels already holding this version finish through the idempotent
        // installation in route order. The first of them is a healthy
        // same-version replica the missing levels are copied from; otherwise
        // the prepared source is. Either way the source is read once.
        let mut replica = None;
        let mut missing = Vec::new();
        for (index, slot) in p.placement.route_slot_ids.iter().enumerate() {
            let disk = self.disk(slot)?;
            let root = &self.disk_resources.get(disk)?.root;
            let directory = version_dir(disk, &p.version.history_tree_id, &p.version.version_id);
            match root
                .read_json_observed::<FileVersion>(&directory.join("version.json"), &mut |_| Ok(()))
            {
                Ok(recorded) => {
                    if object_hash("version", &recorded)? != object_hash("version", &p.version)? {
                        return Err(Error::new(
                            storage_contract::ErrorCode::ObjectCollision,
                            "version identity already exists with different bytes",
                        ));
                    }
                    let verified = match existing.get_mut(index).and_then(Option::take) {
                        Some(verified) => verified,
                        None => VerifiedExisting::read_at(root, &directory, &p.version)?,
                    };
                    verified.assert_binding(&directory, &p.version);
                    let path = crate::container_storage::install_version_content_at(
                        root,
                        &directory,
                        crate::container_storage::PreparedContent::Verified(verified),
                    )?;
                    if replica.is_none() {
                        replica = Some((root, path));
                    }
                }
                Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    let staging = disk
                        .root
                        .join("staging")
                        .join(OperationId::generate().to_string());
                    missing.push(MissingLevel {
                        root,
                        directory,
                        staging,
                    });
                }
                Err(error) => return Err(error),
            }
        }
        if missing.is_empty() {
            return Ok(io);
        }
        let source = match &replica {
            Some((root, path)) => root.open_regular(path)?,
            None => p.source.open()?,
        };
        if missing.len() == 1 || !fan_out_admitted(&p.version, missing.len()) {
            // The original serial chain remains available outside the bounded
            // fan-out envelope; this is not a new product file/route limit.
            let mut source = source;
            for (index, level) in missing.iter().enumerate() {
                level.root.private_dir(&level.staging.join("content"))?;
                let copied = verified_full_copy_at(
                    level.root,
                    source,
                    &level.content(&p.version),
                    &p.version,
                    self.config.max_file_bytes,
                )
                .map_err(CopyFailure::into_error)?;
                let path = level.materialize(&p.version, copied)?;
                io.payload_source_bytes_completed += p.version.logical_size;
                io.logical_bytes_completed += p.version.logical_size;
                if index + 1 < missing.len() {
                    source = level.root.open_regular(&path)?;
                } else {
                    return Ok(io);
                }
            }
        } else {
            // Every missing level is written from the one read, each by its
            // own thread on its own disk, then materialized on that disk.
            let targets: Vec<_> = missing
                .iter()
                .map(|level| FanOutTarget {
                    root: level.root,
                    destination: level.content(&p.version),
                })
                .collect();
            verified_fan_out_copy_at(
                &source,
                &targets,
                &p.version,
                self.config.max_file_bytes,
                |index, copied| missing[index].materialize(&p.version, copied),
            )
            .map_err(CopyFailure::into_error)?;
        }
        io.payload_source_bytes_completed += p.version.logical_size;
        io.logical_bytes_completed += p.version.logical_size * missing.len() as u64;
        Ok(io)
    }
}

/// A route level that does not hold the version yet: its held root, final
/// version directory and private staging directory.
struct MissingLevel<'r> {
    root: &'r DiskRoot,
    directory: std::path::PathBuf,
    staging: std::path::PathBuf,
}

impl MissingLevel<'_> {
    fn content(&self, version: &FileVersion) -> std::path::PathBuf {
        self.staging.join("content").join(&version.creation_name)
    }

    /// Write the version metadata beside the verified copy, sync the staging
    /// directory, move it into place without replacement, sync the parent,
    /// and install the body into the file's named container.
    fn materialize(
        &self,
        version: &FileVersion,
        copied: VerifiedCopy<'_>,
    ) -> Result<std::path::PathBuf> {
        let (root, staging, directory) = (self.root, &self.staging, &self.directory);
        root.write_json(&staging.join("version.json"), version, false)?;
        root.write_atomic(
            &staging.join("checksums.bin"),
            copied.checksum_bytes(),
            false,
        )?;
        root.write_json(
            &staging.join("payload.json"),
            &json!({"payload_id":format!("payload_{}",&version.version_id[4..]),"version_id":version.version_id,"kind":"FULL","size":version.logical_size.to_string(),"content_hash":version.content_hash}),
            false,
        )?;
        root.sync_dir(staging)?;
        root.private_dir(directory.parent().expect("version parent"))?;
        let staging_parent = root.directory(staging.parent().expect("staging parent"))?;
        let version_parent = root.directory(directory.parent().expect("version parent"))?;
        rustix::fs::renameat_with(
            &staging_parent,
            staging.file_name().expect("staging name"),
            &version_parent,
            directory.file_name().expect("version name"),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        root.sync_dir(directory.parent().expect("version parent"))?;
        drop(staging_parent);
        drop(version_parent);
        // Install a real version file in this file's named container before
        // any protection receipt is issued.
        crate::container_storage::install_version_content_at(
            root,
            directory,
            crate::container_storage::PreparedContent::Copied(copied),
        )
    }
}
