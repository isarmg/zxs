//! Durable preparation, decision and publication of one frozen transaction.
use super::{Engine, Prepared};
use crate::journal::RecordType;
use crate::{disk::*, journal::*, *};
use json_codec::json;
use std::collections::BTreeMap;
#[path = "verified_payloads.rs"]
mod verified_payloads;
use verified_payloads::{MAX_HELD_PAYLOAD_FILES, PayloadPreparation, VerifiedPayloadSet};

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
}

enum Completion<'a> {
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
        let hash = object_hash("candidate", &candidate)?;
        self.next_slot = candidate.sequence.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "decision sequence exhausted",
            )
        })?;
        #[cfg(feature = "fault-injection")]
        let mut prepared = 0;
        for d in &members {
            store_object_at(
                &self.disk_resources.get(d)?.root,
                d,
                "candidate",
                &candidate,
            )?;
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
            )
        })();
        if let Err(e) = finish {
            self.metadata_integrity.invalidate();
            let message = format!("{e}; query operation {} and recover", result.operation_id);
            return Err(e
                .context(storage_contract::ErrorCode::InDoubt, message)
                .retryable());
        }
        let previous = std::mem::replace(&mut self.state, std::sync::Arc::new(next));
        self.snapshot = Some(candidate.snapshot_hash.clone());
        self.update_index(&previous);
        // Only the completed member publication establishes this proof. SQLite is
        // a disposable accelerator, not a dependency of the authoritative closure.
        self.published_state_base = metadata_epoch.matches(&self.metadata_integrity).then(|| {
            std::sync::Arc::new(PublishedStateBase {
                state: std::sync::Arc::downgrade(&self.state),
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
        self.complete_candidate_verified(c, hash, next, existing, Completion::Recovery)
    }
    pub(super) fn complete_published_candidate(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        publication: Publication,
        mut append_receipt: impl FnMut(&mut Engine, &DiskConfig, &str) -> Result<Receipt>,
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
        let pub_hash = object_hash("publication", &publication)?;
        let mut publication_receipts = Vec::new();
        for d in self.members() {
            store_object_at(
                &self.disk_resources.get(&d)?.root,
                &d,
                "publication",
                &publication,
            )?;
            publication_receipts.push(append_receipt(self, &d, &pub_hash)?);
            #[cfg(feature = "fault-injection")]
            if publication.recovery.is_some() {
                self.failpoint(&format!(
                    "after_recovery_publication_{}",
                    publication_receipts.len()
                ))?;
            }
        }
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
                match handle.root.directory(&dir) {
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
        let final_hash = object_hash("finalize", &finalize)?;
        let mut final_receipts = Vec::new();
        for d in &members {
            store_object_at(&self.disk_resources.get(d)?.root, d, "finalize", &finalize)?;
            final_receipts.push(append(
                self,
                d,
                RecordType::Finalize,
                c.sequence,
                hash,
                &final_hash,
            )?);
        }
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_finalize")?;
        // Only a decided candidate may rename a published file container.
        // A crash leaves the operation indeterminate and recovery replays it.
        names.apply()?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_container_names")?;
        let mut payload_receipts = Vec::new();
        for evidence in &c.versions {
            for slot in &evidence.placement.route_slot_ids {
                let d = self.disk(slot)?;
                let handle = self.disk_resources.get(d)?;
                let directory = version_dir(
                    d,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                if let Some(verified) = verified {
                    verified.validate_payload(&self.config, &evidence.version, slot)?;
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
        let pub_hash = object_hash("publication", &publication)?;
        let mut publication_receipts = Vec::new();
        for d in &members {
            store_object_at(
                &self.disk_resources.get(d)?.root,
                d,
                "publication",
                &publication,
            )?;
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
                let proof =
                    root.read_json_observed(&proof_dir.join("commit-proof.json"), &mut |_| Ok(()))?;
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
    let original = match root.open_regular(path) {
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
            self.prepare_payload_with_existing(payload, copies)
        })
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn prepare_payload(&self, p: &Prepared) -> Result<()> {
        self.prepare_payload_with_existing(p, Vec::new())
    }

    fn prepare_payload_with_existing<'a>(
        &self,
        p: &'a Prepared,
        mut existing: ExistingCopies<'a>,
    ) -> Result<()> {
        let mut source = None;
        for (index, slot) in p.placement.route_slot_ids.iter().enumerate() {
            let disk = self.disk(slot)?;
            let handle = self.disk_resources.get(disk)?;
            let root = &handle.root;
            let directory = version_dir(disk, &p.version.history_tree_id, &p.version.version_id);
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
                    let verified = match existing.get_mut(index).and_then(Option::take) {
                        Some(verified) => verified,
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
                        .join(OperationId::generate().to_string());
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
                    let staging_parent =
                        root.directory(staging.parent().expect("staging parent"))?;
                    let version_parent =
                        root.directory(directory.parent().expect("version parent"))?;
                    rustix::fs::renameat_with(
                        &staging_parent,
                        staging.file_name().expect("staging name"),
                        &version_parent,
                        directory.file_name().expect("version name"),
                        rustix::fs::RenameFlags::NOREPLACE,
                    )
                    .map_err(std::io::Error::from)?;
                    root.sync_dir(directory.parent().expect("version parent"))?;
                    crate::container_storage::PreparedContent::Copied(copied)
                }
                Err(error) => return Err(error),
            };
            // Install a real version file in this file's named container before
            // any protection receipt is issued. The next level reads this copy.
            let path =
                crate::container_storage::install_version_content_at(root, &directory, content)?;
            if index + 1 < p.placement.route_slot_ids.len() {
                source = Some(root.open_regular(&path)?);
            }
        }
        Ok(())
    }
}
