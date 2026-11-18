//! Verify published receipts before workspace admission or recovery writes.
use super::*;
use crate::engine::disk_registry::current::CurrentNamespaces;
use std::sync::Arc;

mod content;
mod directory;
pub(crate) use directory::DirectoryReplayPrefix;
pub(super) use directory::CheckpointDirectoryObservation;

mod installed;
pub(crate) use installed::InstalledOriginalReceipts;
use installed::ReceiptEnvironment;

/// A loan of the actual no-repair replay branch's original snapshot and complete
/// Publication barrier. It has no constructor, writer, installer or Current
/// conversion outside this module. It only permits logical source reconstruction
/// without reopening an unavailable upstream device.
pub(crate) struct ReadOnlyPublishedCandidate<'a> {
    previous: &'a State,
    next: &'a State,
    candidate: &'a Candidate,
    config: &'a ValidatedConfig,
    resources: &'a DiskResources,
    snapshot: &'a Option<String>,
    _publication: &'a Publication,
    _seal: &'a PublicationSeal,
}
impl ReadOnlyPublishedCandidate<'_> {
    pub(crate) fn require_context(
        &self,
        context: &crate::engine::PlanningContext<'_>,
    ) -> Result<()> {
        if !std::ptr::eq(self.previous, context.state.as_ref())
            || !std::ptr::eq(self.config, context.config)
            || !std::ptr::eq(self.resources, context.disk_resources.as_ref())
            || self.snapshot != context.snapshot
            || self.candidate.previous_snapshot != *self.snapshot
        {
            return Err(Error::AutomaticBackup(crate::protection::AutomaticBackupError::Source));
        }
        Ok(())
    }

    pub(crate) fn require_pair(
        &self,
        context: &crate::engine::PlanningContext<'_>,
        next: &State,
        candidate: &Candidate,
    ) -> Result<()> {
        self.require_context(context)?;
        if !std::ptr::eq(self.next, next) || !std::ptr::eq(self.candidate, candidate) {
            return Err(Error::AutomaticBackup(
                crate::protection::AutomaticBackupError::Publication,
            ));
        }
        Ok(())
    }
}

pub(super) struct PreparedCandidate {
    pub candidate: Candidate,
    pub publication: Option<Publication>,
    pub barrier_failure: Option<Error>,
    // Only this exact candidate.snapshot_hash, consumed once under the same
    // admitted roots/locks. Never an all-history State cache.
    pub snapshot: Option<crate::publication_evidence::SharedAdmittedSnapshot>,
    // A pure next view for this preflight slot, never an Engine installation.
    next_namespaces: Option<CurrentNamespaces>,
    directory_admission: Option<directory::DirectoryCandidateAdmission>,
}

/// The one source-backed directory index needs the original for every indexed
/// receipt, not just its latest kind. This cache contains no State, material,
/// format or writer authority; each entry came from the actual barrier below
/// and must still match the requested visible prefix.
#[derive(Clone, Default)]
pub(crate) struct OriginalReceiptCursor {
    environment: Option<ReceiptEnvironment>,
    records: BTreeMap<OperationId, Arc<crate::publication_evidence::OriginalReceiptProjection>>,
}

/// A read-only reborrow after every reached original/outcome pair was checked
/// against this prefix. Missing entries remain missing; this is not a State or
/// publication issuer and cannot supply archived originals by decoding State.
pub(crate) struct OriginalReceiptPrefix<'a> {
    source: &'a State,
    records: BTreeMap<OperationId, (&'a OperationResult, &'a storage_contract::Principal)>,
}

impl<'a> OriginalReceiptPrefix<'a> {
    pub(crate) fn source(&self) -> &'a State { self.source }

    pub(crate) fn receipt(
        &self, operation: OperationId,
    ) -> Option<(&'a OperationResult, &'a storage_contract::Principal)> {
        self.records.get(&operation).copied()
    }
}

impl OriginalReceiptCursor {
    pub(in crate::engine) fn for_recovery(engine: &Engine) -> Result<Self> {
        if let Some(generation) = engine.config.generation()
            && Arc::ptr_eq(&engine.state, &generation.base)
        {
            if engine.snapshot != generation.snapshot() {
                return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
            }
            // A generation installer changes the admitted resources as well
            // as its State. Reobserve its authentic retained archive, not the
            // previous environment's installed cursor or a State-only index.
            let indexed = engine.state.operations.values().any(
                crate::snapshot_worktree::ReceiptIndex::requires_original,
            );
            let cursor = if indexed {
                engine.replay_generation_original_receipts(generation)?
            } else {
                Self::default()
            };
            cursor.require_directory_index(&engine.state)?;
            return Ok(cursor);
        }
        // Preserve the ordinary replay ordering: preflight validates missing
        // originals before any writer conversion. An empty cursor cannot be
        // installed when that prefix contains indexed duties.
        match &engine.original_receipts {
            Some(source) => source.replay_cursor(&engine.planning_context()),
            None => Ok(Self::default()),
        }
    }

    pub(in crate::engine) fn retain_archive(
        &mut self,
        source: &crate::engine::membership_transition::VerifiedArchivePublication,
        visible: &State,
        config: &ValidatedConfig,
        resources: &Arc<DiskResources>,
    ) -> Result<()> {
        let projection = crate::publication_evidence::OriginalReceiptProjection::from_archive(
            source, visible,
        )?;
        // Unlike an observation of the same live barrier, two archive slots
        // may not repeat an operation, even with an equal result.
        self.retain(projection, config, resources)
    }

    pub(crate) fn for_context(context: &crate::engine::PlanningContext<'_>)
        -> Result<Self>
    {
        match context.original_receipts {
            Some(source) => source.replay_cursor(context),
            None => {
                let empty = Self::default();
                empty.require_directory_index(context.state)?;
                Ok(empty)
            }
        }
    }

    pub(in crate::engine) fn needs_projection(&self, result: &OperationResult) -> bool {
        self.records.is_empty()
            || result.commit_state == storage_contract::CommitState::PublishedDegraded
            || crate::snapshot_worktree::ReceiptIndex::requires_original(result)
    }

    pub(in crate::engine) fn retain_completed(
        &mut self, projection: crate::publication_evidence::OriginalReceiptProjection,
        config: &ValidatedConfig, resources: &Arc<DiskResources>,
    ) -> Result<()> {
        if let Some(prior) = self.records.get(&projection.operation()) {
            self.environment.as_ref()
                .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix))?
                .require(config, resources)?;
            // The actual barrier/completion projected this pair again. A
            // repeated observation may retain it, but cannot replace either
            // its original receipt, actor or visible recovery outcome.
            if prior.as_ref() != &projection {
                return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
            }
            return Ok(());
        }
        self.retain(projection, config, resources)
    }

    fn retain(
        &mut self, projection: crate::publication_evidence::OriginalReceiptProjection,
        config: &ValidatedConfig, resources: &Arc<DiskResources>,
    ) -> Result<()> {
        let operation = projection.operation();
        if self.records.contains_key(&operation) {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
        }
        match &self.environment {
            Some(environment) => environment.require(config, resources)?,
            None => self.environment = Some(ReceiptEnvironment::capture(config, resources)),
        }
        self.records.insert(operation, Arc::new(projection));
        Ok(())
    }

    pub(crate) fn receipt<'a>(
        &'a self, prefix: &State, operation: OperationId,
    ) -> Result<(&'a OperationResult, &'a storage_contract::Principal)> {
        self.records.get(&operation)
            .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix))?
            .in_prefix(prefix)
    }

    /// Recovery streams the prefix a second time after preflight. Projections
    /// for later slots are not part of an earlier prefix; every reached slot
    /// must still contain exactly the retained visible outcome and actor.
    pub(crate) fn require_prefix(&self, prefix: &State) -> Result<()> {
        self.visit_prefix(prefix, |_, _, _| {})
    }

    pub(crate) fn reborrow<'a>(&'a self, prefix: &'a State) -> Result<OriginalReceiptPrefix<'a>> {
        let mut records = BTreeMap::new();
        self.visit_prefix(prefix, |operation, original, principal| {
            records.insert(operation, (original, principal));
        })?;
        Ok(OriginalReceiptPrefix { source: prefix, records })
    }

    fn visit_prefix<'a>(
        &'a self, prefix: &State,
        mut visit: impl FnMut(OperationId, &'a OperationResult, &'a storage_contract::Principal),
    ) -> Result<()> {
        for projection in self.records.values().filter(|projection| {
            projection.sequence().get() <= prefix.sequence
        }) {
            let (original, principal) = projection.in_prefix(prefix)?;
            visit(projection.operation(), original, principal);
        }
        Ok(())
    }

    fn require_directory_index(&self, prefix: &State) -> Result<()> {
        let indexed = prefix.operations.values().any(
            crate::snapshot_worktree::ReceiptIndex::requires_original,
        );
        let retained = self.records.values().any(|projection| {
            projection.sequence().get() <= prefix.sequence
        });
        if !indexed && !retained {
            // There are no directory records to index. This is absence of the
            // feature, not a fallback decoder for a different pool format.
            return Ok(());
        }
        crate::snapshot_worktree::ReceiptIndex::capture_with_originals(prefix, self)
            .map(|_| ())
            .map_err(|source| {
                crate::snapshot_worktree::drafts::DirectoryPublicationError::ReceiptIndex(
                    source.into(),
                ).into_engine()
            })
    }
}

pub(super) struct ReceiptPrefix<'a> {
    pub snapshot: &'a Option<String>,
    pub state: &'a State,
    pub namespaces: Option<&'a CurrentNamespaces>,
    pub originals: &'a OriginalReceiptCursor,
}

impl Engine {
    pub(in crate::engine) fn preflight_workspace_receipts(
        &mut self,
        prepared: &mut PreparedRecovery,
    ) -> Result<()> {
        let mut previous = self.snapshot.clone();
        let mut previous_state = Arc::clone(&self.state);
        let mut namespaces = self.current_namespaces.as_deref().cloned();
        let mut published = None;
        // Any archive seed carrying indexed directory records must also carry
        // their real originals. Visible words or Config equality cannot
        // recreate them, even for an earlier operation kind.
        prepared.original_receipts.require_directory_index(&previous_state)?;
        let mut content_fences = if previous_state.operations.values().any(|r| r.kind == crate::persist::snapshot_worktree::CONTENT_RESERVE) {
            crate::snapshot_worktree::ReceiptIndex::capture_with_originals(&previous_state, &prepared.original_receipts)
                .map_err(crate::business::file_draft::worktree::failure)?.content_replay_fences()
        } else { crate::snapshot_worktree::ContentReplayFences::default() };
        for (sequence, candidates) in &prepared.records {
            let prepared_content = if candidates.values().any(|records|
                records.iter().all(|r| r.record.record_type == RecordType::Prepare)) {
                self.original_content_prepare(&previous, &prepared.online, *sequence, candidates, &content_fences)?
            } else { None };
            if let Some(hash) = &prepared_content {
                if prepared.original_content_prepare.is_some() || prepared.records.range((
                    std::ops::Bound::Excluded(*sequence), std::ops::Bound::Unbounded)).next().is_some() {
                    return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
                }
                prepared.original_content_prepare = Some((*sequence, hash.clone()));
            }
            let mut active = candidates.iter().filter(|(hash, receipts)| {
                self.move_completion.as_ref().is_some_and(|selected| {
                    selected.sequence() == *sequence && selected.candidate_hash() == hash.as_str()
                }) || prepared_content.as_ref() == Some(*hash) || receipts.iter().any(|receipt| {
                    matches!(
                        receipt.record.record_type,
                        RecordType::Accept | RecordType::Publication
                    )
                })
            });
            let Some((hash, receipts)) = active.next() else {
                continue;
            };
            if active.next().is_some() {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "different accepted candidates occupy the same slot",
                ));
            }
            let mut candidate = self.read_workspace_receipt_candidate(
                &prepared.online,
                *sequence,
                hash,
                receipts,
                ReceiptPrefix {
                    snapshot: &previous,
                    state: &previous_state,
                    namespaces: namespaces.as_ref(),
                    originals: &prepared.original_receipts,
                },
            )?;
            if !content_fences.is_empty() {
                let next = candidate.snapshot.as_ref().ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix))?.share_state();
                let result = next.operations.get(&candidate.candidate.operation_id)
                    .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
                content_fences.require_next(&next, result).map_err(crate::business::file_draft::worktree::failure)?;
            }
            if self.move_completion.as_ref().is_some_and(|selected| {
                selected.sequence() == *sequence && selected.candidate_hash() == hash
            }) {
                // Publication alone is not the move serving-scope barrier.
                // Keep the original predecessor for workspace admission.
                prepared.candidates.insert(*sequence, candidate);
                break;
            }
            let barrier = match &candidate.publication {
                Some(publication) => published_barrier(
                    self,
                    &prepared.online,
                    receipts,
                    &candidate.candidate,
                    publication,
                ),
                None => {
                    let admitted = match candidate.snapshot.take() {
                        Some(snapshot) => snapshot.into_admitted(),
                        None => verified_recovery_snapshot_at(
                            &self.disk_resources, &prepared.online,
                            &candidate.candidate.snapshot_hash,
                        )?,
                    };
                    prepared.container_locations.admit_finalized(
                        self, &prepared.online, receipts, &previous_state,
                        &admitted, &candidate.candidate, candidate.publication.as_ref(),
                    )?;
                    candidate.snapshot = Some(admitted.into_shared());
                    prepared.candidates.insert(*sequence, candidate);
                    break;
                }
            };
            let seal = match barrier {
                Ok(seal) => seal,
                Err(source) => {
                    // This slot may still be repairable by apply. It cannot grant
                    // cold admission. Keep the concrete failure for diagnostics.
                    let admitted = match candidate.snapshot.take() {
                        Some(snapshot) => snapshot.into_admitted(),
                        None => verified_recovery_snapshot_at(
                            &self.disk_resources, &prepared.online,
                            &candidate.candidate.snapshot_hash,
                        )?,
                    };
                    prepared.container_locations.admit_finalized(
                        self, &prepared.online, receipts, &previous_state,
                        &admitted, &candidate.candidate, candidate.publication.as_ref(),
                    )?;
                    candidate.snapshot = Some(admitted.into_shared());
                    candidate.barrier_failure = Some(source);
                    prepared.candidates.insert(*sequence, candidate);
                    break;
                }
            };
            if candidate.publication.as_ref().is_some_and(|publication| {
                publication.result.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND
            }) {
                super::super::move_execution::CompletionBinding::require_completed(
                    self,
                    &candidate.candidate,
                    hash,
                )?;
            }
            let registry = candidate.candidate.registry().is_some()
                || self.is_backup_first_candidate(&candidate.candidate)?;
            if registry {
                if candidate.next_namespaces.is_none()
                    && let Err(source) =
                        self.preflight_registry_barrier(&prepared.online, receipts, &seal)
                {
                    // A serialized seal is not an actual D3 journal barrier.
                    // Leave this slot for normal repair, without inspecting any
                    // later payload proof or advancing workspace admission.
                    candidate.barrier_failure = Some(source);
                    prepared.candidates.insert(*sequence, candidate);
                    break;
                }
            }
            // Every published predecessor supplies the actual namespace source
            // for the following slot's source-scoped local-proof discovery.
            // Retain the authenticated original, never reconstruct it from next.
            let admitted = match candidate.snapshot.take() {
                Some(snapshot) => snapshot.into_admitted(),
                None => verified_recovery_snapshot_at(
                    &self.disk_resources,
                    &prepared.online,
                    &candidate.candidate.snapshot_hash,
                )?,
            };
            let original = admitted.for_candidate(&candidate.candidate)?;
            verify_current_candidate_receipt(&candidate.candidate, original.state())?;
            if registry {
                namespaces = Some(match candidate.next_namespaces.take() {
                    Some(view) => view,
                    None if self.is_backup_first_candidate(&candidate.candidate)? => self
                        .preflight_backup_first_current(
                            &candidate.candidate,
                            original,
                            candidate
                                .publication
                                .as_ref()
                                .expect("invariant: published barrier"),
                            &seal,
                        )?,
                    None => self.preflight_registry_activation(
                        &candidate.candidate,
                        original,
                        candidate
                            .publication
                            .as_ref()
                            .expect("invariant: published barrier"),
                        &seal,
                    )?,
                });
            }
            prepared.container_locations.admit_published(
                &self.config, &previous_state, &admitted, &candidate.candidate,
                candidate.publication.as_ref().expect("invariant: published barrier"), &seal,
            )?;
            let retain_original = original.state().operations
                .get(&candidate.candidate.operation_id)
                .is_some_and(|result| prepared.original_receipts.needs_projection(result))
                || candidate.publication.as_ref().is_some_and(|p| p.recovery.is_some());
            let result = original.state().operations.get(&candidate.candidate.operation_id)
                .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
            content_fences.advance(result).map_err(crate::business::file_draft::worktree::failure)?;
            let shared = admitted.into_shared();
            previous_state = shared.share_state();
            // The original proof remains unmodified for this candidate.
            // Only the next slot's predecessor includes an authenticated
            // recovered outcome, just as the visible replay state does.
            if let Some(publication) = &candidate.publication
                && publication.recovery.is_some()
            {
                Arc::make_mut(&mut previous_state)
                    .operations
                    .insert(candidate.candidate.operation_id, publication.result.clone());
            }
            if retain_original {
                // This is deliberately after the actual held journal barrier
                // and the legacy original/visible construction above. Never
                // infer an original by stripping fields from visible results.
                let projection = shared.project_original_receipt(
                    &candidate.candidate,
                    candidate.publication.as_ref().expect("invariant: published barrier"),
                    &seal,
                    &previous_state,
                )?;
                prepared.original_receipts.retain_completed(
                    projection, &self.config, &self.disk_resources,
                )?;
            }
            // Retain only the compact already-admitted native observation.
            // A checkpoint may skip installing historical predecessor States;
            // it must not replay this transition against that stale Engine State.
            if let Some(admission) = candidate.directory_admission.take() {
                let observed = admission.into_checkpoint(
                    self, &candidate.candidate,
                    candidate.publication.as_ref().expect("invariant: published barrier"),
                    &seal,
                )?;
                prepared.directory_checkpoints.insert(*sequence, observed);
            }
            candidate.snapshot = Some(shared);
            previous = Some(candidate.candidate.snapshot_hash.clone());
            published = Some(*sequence);
            // Keep only the latest proven slot, plus at most the first pending
            // slot. Earlier candidates are streamed again for historical repair.
            prepared.candidates.clear();
            prepared.candidates.insert(*sequence, candidate);
        }
        // Build the complete relation once over the last verified visible
        // prefix. Scoped head/retry lookups cannot skip unrelated bad records.
        prepared.original_receipts.require_directory_index(&previous_state)?;
        // End the cursor's snapshot loan before reclaiming the latest cached
        // admitted State below (SharedAdmittedState requires unique ownership).
        drop(previous_state);
        self.verify_membership_fence_prefix(published, previous.as_deref())?;
        // Main's operation and operation_principals maps remain authoritative.
        // Only the latest fully published State is needed for cold admission;
        // no alternate receipt index or persistent representation is introduced.
        let state = if let Some(sequence) = published {
            let bound = prepared
                .candidates
                .get_mut(&sequence)
                .expect("invariant: preflight retains its latest published candidate");
            let admitted = match bound.snapshot.take() {
                Some(snapshot) => snapshot.into_admitted(),
                None => verified_recovery_snapshot_at(
                    &self.disk_resources,
                    &prepared.online,
                    &bound.candidate.snapshot_hash,
                )?,
            };
            let state = admitted.for_candidate(&bound.candidate)?.state();
            let result = state
                .operations
                .get(&bound.candidate.operation_id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ObjectCorrupt,
                        "published candidate result is absent",
                    )
                })?;
            if state.sequence != sequence
                || result.operation_id != bound.candidate.operation_id
                || result.sequence != sequence
                || result.request_digest != bound.candidate.request_digest
            {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "published snapshot receipt differs",
                ));
            }
            // Cold admission needs the original actor/request/result bindings.
            // Apply retains the original admitted proof and overlays any degraded
            // result only after checking the candidate-bound history projection.
            let shared = admitted.into_shared();
            let state = shared.share_state();
            bound.snapshot = Some(shared);
            state
        } else {
            Arc::clone(&self.state)
        };
        // The seed is read-only and borrows this verified publication view only
        // for admission. Replay must still start from its original prefix.
        let original = self.named_namespaces.install_state(&mut self.state, state);
        let admission = self.seed_workspace_recovery();
        self.named_namespaces.install_state(&mut self.state, original);
        admission
    }

    pub(super) fn read_workspace_receipt_candidate(
        &self,
        online: &[DiskConfig],
        sequence: u64,
        hash: &str,
        receipts: &[Receipt],
        prefix: ReceiptPrefix<'_>,
    ) -> Result<PreparedCandidate> {
        let candidate: Candidate =
            verified_object_at(&self.disk_resources, online, "candidate", hash)?;
        self.verify_candidate_membership(&candidate)?;
        if let Some(generation) = self.config.generation()
            && prefix.snapshot == &generation.snapshot()
        {
            generation.verify_activation_candidate(&candidate)?;
        }
        if candidate.sequence != sequence
            || candidate.pool_id != self.config.pool_id
            || &candidate.previous_snapshot != prefix.snapshot
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "candidate does not extend the verified publication prefix",
            ));
        }
        prefix.originals.require_prefix(prefix.state)?;
        let registry = candidate.registry().is_some() || self.is_backup_first_candidate(&candidate)?;
        let read_publication = || -> Result<Option<Publication>> {
            receipts.iter().find(|receipt| {
                receipt.record.record_type == RecordType::Publication
            }).map(|receipt| verified_object_at(
                &self.disk_resources, online, "publication", &receipt.record.object_hash,
            )).transpose()
        };
        // Ordinary published slots historically read this global object before
        // their snapshot. Preserve that failure precedence; no material source
        // or visible-state authority is obtained from the object by itself.
        let journal_publication = if registry { None } else { read_publication()? };
        let admitted = verified_recovery_snapshot_at(
            &self.disk_resources, online, &candidate.snapshot_hash,
        )?;
        if admitted.for_candidate(&candidate)?.state().operations
            .get(&candidate.operation_id).is_some_and(|result| {
                result.kind == crate::persist::directory_draft_commit::native::KIND
            })
        {
            return self.read_directory_receipt_candidate(
                candidate, admitted, journal_publication, hash, prefix,
            );
        }
        let content_reserve = admitted.for_candidate(&candidate)?.state().operations.get(&candidate.operation_id)
            .is_some_and(|r| r.kind == crate::persist::snapshot_worktree::CONTENT_RESERVE);
        if content_reserve {
            self.require_content_reservation(&candidate, &prefix, &admitted)?;
        }
        if admitted.for_candidate(&candidate)?.state().operations.get(&candidate.operation_id)
            .is_some_and(|r| r.kind == crate::persist::snapshot_worktree::CONTENT_PUBLISH)
        {
            return self.read_content_receipt_candidate(candidate, admitted, journal_publication, prefix, online, hash, receipts);
        }
        // Even ACCEPT-only discovery may open a data-only local proof. Prove
        // this slot against its exact admitted predecessor before that search;
        // receipt labels alone cannot grant permission to inspect payloads.
        let mut next_namespaces = None;
        if registry {
            let original = admitted.for_candidate(&candidate)?;
            crate::history_package::bound_history_objects_at(
                &self.disk_resources,
                online,
                &candidate,
                Some(original),
            )?;
            if let Some(view) = prefix.namespaces {
                next_namespaces = Some(verify_current_candidate_transition(
                    view,
                    prefix.state,
                    original.state(),
                    &candidate,
                )?);
            } else {
                // The exact prescribed activation has no payload vector. Only
                // its verified publication barrier can establish the pure view
                // used to check following slots. Pending activation grants none.
                let generation = self.config.generation().ok_or_else(|| {
                    Error::new(
                        ErrorCode::PublicationInvalid,
                        "registry candidate has no generation",
                    )
                })?;
                if prefix.snapshot != &generation.snapshot() {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "registry candidate precedes its verified activation",
                    ));
                }
                generation.verify_activation_candidate(&candidate)?;
            }
        }
        let journal_publication = if registry { read_publication()? } else { journal_publication };
        let publication = match journal_publication {
            Some(publication) => Some(publication),
            None => crate::degraded_recovery::discover_publication(
                self, &candidate, hash, prefix.state, prefix.snapshot, &admitted,
            )?,
        };
        let snapshot = Some(admitted.into_shared());
        if publication
            .as_ref()
            .is_some_and(|publication| publication.recovery.is_some())
            && let Some(snapshot) = &snapshot
        {
            CurrentNamespaces::reject_degraded_backup(&candidate, &snapshot.share_state())?;
            CurrentNamespaces::reject_degraded_history_copy(&candidate, &snapshot.share_state())?;
            CurrentNamespaces::reject_degraded_history_move(&candidate, &snapshot.share_state())?;
        }
        // The reserve also reconstructed its *entire* original no-content
        // transition above. A checkpoint may reborrow its authenticated barrier
        // but must not try it again against a skipped predecessor State.
        let directory_admission = if content_reserve {
            publication.as_ref().map(|p| directory::DirectoryCandidateAdmission::capture(self, &candidate, p)).transpose()?
        } else { None };
        Ok(PreparedCandidate {
            candidate,
            publication,
            barrier_failure: None,
            snapshot,
            next_namespaces,
            directory_admission,
        })
    }

    fn preflight_registry_barrier(
        &self,
        online: &[DiskConfig],
        receipts: &[Receipt],
        seal: &PublicationSeal,
    ) -> Result<()> {
        if online.len() < self.config.metadata_quorum() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "registry preflight lacks its unchanged target metadata quorum",
            ));
        }
        if online.iter().any(|disk| {
            !seal.receipts.iter().any(|sealed| {
                sealed.disk_id == disk.disk_id
                    && sealed.slot_id == disk.slot_id
                    && receipts.iter().any(|actual| actual == sealed)
            })
        }) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "registry preflight lacks an actual retained D3 publication receipt",
            ));
        }
        Ok(())
    }

    /// Admit only the prescribed first candidate for repair, not for Current.
    /// Its publication may still lack D3 receipts. Completion and live admission
    /// independently require the full barrier before installing a namespace.
    pub(in crate::engine) fn verify_replay_namespace_candidate(
        &self,
        candidate: &Candidate,
        original: &crate::history_package::AdmittedState,
        publication: Option<&Publication>,
    ) -> Result<()> {
        self.verify_replay_namespace_source(candidate, original, publication, None, None)
    }

    /// Borrow the cursor that already admitted the actual later publications.
    /// It changes only a physical historical read location, never its original
    /// State, receipt, namespace or immutable extent identities.
    pub(super) fn verify_replay_namespace_at_locations(
        &self,
        candidate: &Candidate,
        original: &crate::history_package::AdmittedState,
        publication: Option<&Publication>,
        seal: Option<&PublicationSeal>,
        locations: &super::super::fresh_layout::containers::RecoveryRenameLocations<'_>,
    ) -> Result<()> {
        match (publication, seal) {
            (Some(publication), Some(seal)) => self.verify_readonly_published_namespace_source(
                candidate, original, publication, seal, Some(locations),
            ),
            _ => self.verify_replay_namespace_source(
                candidate, original, publication, None, Some(locations),
            ),
        }
    }

    /// Called only from recover's !can_repair branch after the same actual seal
    /// observation that otherwise guarded read-only visible installation below.
    pub(super) fn verify_readonly_published_namespace_candidate(
        &self,
        candidate: &Candidate,
        original: &crate::history_package::AdmittedState,
        publication: &Publication,
        seal: &PublicationSeal,
    ) -> Result<()> {
        self.verify_readonly_published_namespace_source(
            candidate, original, publication, seal, None,
        )
    }

    fn verify_readonly_published_namespace_source(
        &self,
        candidate: &Candidate,
        original: &crate::history_package::AdmittedState,
        publication: &Publication,
        seal: &PublicationSeal,
        locations: Option<&super::super::fresh_layout::containers::RecoveryRenameLocations<'_>>,
    ) -> Result<()> {
        // The caller retains PreparedRecovery's unchanged fixed-member quorum;
        // this source cannot enter the pending or can_repair branches. A
        // degraded recovery keeps its original strict source path: only normal
        // all-member ACCEPT/FINALIZE/Publication evidence issues this loan.
        if publication.recovery.is_some() {
            return self.verify_replay_namespace_source(
                candidate, original, Some(publication), None, locations,
            );
        }
        self.verify_candidate_membership(candidate)?;
        // The one seal kernel also verifies this Candidate's complete original
        // ACCEPT/FINALIZE set; a local Publication object is not enough.
        verify_publication_seal(candidate, publication, seal)?;
        verify_current_candidate_receipt(candidate, original.state())?;
        let source = ReadOnlyPublishedCandidate {
            previous: self.state.as_ref(),
            next: original.state(),
            candidate,
            config: &self.config,
            resources: self.disk_resources.as_ref(),
            snapshot: &self.snapshot,
            _publication: publication,
            _seal: seal,
        };
        source.require_pair(&self.planning_context(), original.state(), candidate)?;
        self.verify_replay_namespace_source(
            candidate, original, Some(publication), Some(&source), locations,
        )
    }

    fn verify_replay_namespace_source(
        &self,
        candidate: &Candidate,
        original: &crate::history_package::AdmittedState,
        publication: Option<&Publication>,
        readonly: Option<&ReadOnlyPublishedCandidate<'_>>,
        locations: Option<&super::super::fresh_layout::containers::RecoveryRenameLocations<'_>>,
    ) -> Result<()> {
        let activation = self.config.generation().filter(|generation| {
            self.current_namespaces.is_none()
                && generation.registry().is_some()
                && !generation.portable.is_backup()
        });
        let Some(generation) = activation else {
            return self.verify_current_recovery_candidate_source(
                candidate, original.state(), readonly, locations,
            );
        };
        if self.snapshot != generation.snapshot()
            || self.state.as_ref() != generation.base.as_ref()
        {
            return Err(crate::engine::disk_registry::current::pending_publication());
        }
        generation.verify_activation_candidate(candidate)?;
        if let Some(publication) = publication {
            crate::journal::verify_finalized_publication(candidate, publication)?;
            generation.verify_activation_result(publication)?;
        }
        // No namespace or seal is created here. Normal recovery repairs the
        // exact receipts, then restore_current_disk_exit re-reads the full D3.
        Ok(())
    }

    pub(crate) fn verify_current_recovery_candidate(
        &self,
        candidate: &Candidate,
        next: &State,
    ) -> Result<()> {
        self.verify_current_recovery_candidate_source(candidate, next, None, None)
    }

    fn verify_current_recovery_candidate_source(
        &self,
        candidate: &Candidate,
        next: &State,
        readonly: Option<&ReadOnlyPublishedCandidate<'_>>,
        locations: Option<&super::super::fresh_layout::containers::RecoveryRenameLocations<'_>>,
    ) -> Result<()> {
        let containers =
            super::super::fresh_layout::containers::verify_transition_with_working_source(
                &self.config, &self.disk_resources, &self.state, next, candidate, locations,
            )?;
        match readonly {
            Some(source) => crate::protection::verify_readonly_automatic_backup_candidate(
                &self.planning_context(), next, candidate, &containers, source,
            )?,
            None => crate::protection::verify_automatic_backup_candidate(
                &self.planning_context(), next, candidate, &containers,
            )?,
        }
        if let Some(view) = self.admitted_current_namespaces()? {
            verify_current_candidate_transition(view, &self.state, next, candidate)?;
        } else {
            crate::engine::disk_registry::current::verify_file_metadata(
                &self.state, next, None, None,
            )?;
            crate::engine::disk_registry::current::verify_path_changes(
                &self.state,
                next,
                None,
                None,
            )?;
            crate::engine::disk_registry::current::verify_container_tags(
                &self.state,
                next,
                None,
                None,
            )?;
            if next
                .operations
                .get(&candidate.operation_id)
                .is_some_and(|receipt| {
                    receipt.kind == storage_contract::file_container_tags::KIND
                        || crate::persist::path_changes::is_kind(&receipt.kind)
                        || crate::persist::file_metadata::is_kind(&receipt.kind)
                })
            {
                verify_current_candidate_receipt(candidate, next)?;
            }
        }
        Ok(())
    }
}

fn published_barrier(
    engine: &Engine,
    online: &[DiskConfig],
    receipts: &[Receipt],
    candidate: &Candidate,
    publication: &Publication,
) -> Result<PublicationSeal> {
    let publication_hash = object_hash("publication", publication)?;
    let complete = candidate
        .metadata_disk_ids
        .iter()
        .map(|id| {
            receipts
                .iter()
                .find(|receipt| {
                    receipt.disk_id == *id
                        && receipt.record.record_type == RecordType::Publication
                        && receipt.record.object_hash == publication_hash
                })
                .cloned()
        })
        .collect::<Option<Vec<_>>>();
    if let Some(receipts) = complete {
        let seal = PublicationSeal {
            candidate_hash: publication.candidate_hash.clone(),
            publication_hash,
            receipts,
        };
        verify_publication_seal(candidate, publication, &seal)?;
        Ok(seal)
    } else {
        verified_publication_seal_at(&engine.disk_resources, online, candidate, publication)
    }
}

// All callers share this ordering, including explicit degraded recovery. This
// builds a pure view; neither it nor the preflight activation authorizes live
// namespace installation, payload repair or weakening a publication barrier.
fn verify_current_candidate_transition(
    view: &CurrentNamespaces,
    previous: &State,
    next: &State,
    candidate: &Candidate,
) -> Result<CurrentNamespaces> {
    let advanced = view.advance(previous, next)?;
    verify_current_candidate_receipt(candidate, next)?;
    if !view.verify_backup_candidate_payloads(previous, next, candidate)?
        && !view.verify_history_copy_candidate(previous, next, candidate)?
        && !view.verify_history_move_candidate(previous, next, candidate)?
    {
        super::payload_admission::verify(view, &advanced, previous, next, candidate)?;
    }
    Ok(advanced)
}

// State-only transition proofs cannot constrain Candidate.versions, which
// separately drives payload repair and local proof reconstruction. The receipt
// here is always from the original authenticated snapshot, never a recovered
// publication's outcome overlay. Full current-view write-set checks still run.
pub(super) fn verify_current_candidate_receipt(candidate: &Candidate, next: &State) -> Result<()> {
    let result = next
        .operations
        .get(&candidate.operation_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "current-registry candidate has no original operation receipt",
            )
        })?;
    if next.sequence != candidate.sequence
        || result.operation_id != candidate.operation_id
        || result.sequence != candidate.sequence
        || result.request_digest != candidate.request_digest
    {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "current-registry candidate differs from its original operation receipt",
        ));
    }
    if matches!(
        result.kind.as_str(),
        "plan.cancel" | "branch.archive" | "merge-link.create" | "file.container-tags.replace"
            | "file.metadata" | "version.metadata" | "file.branch.rename"
    ) && !candidate.versions.is_empty()
    {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "metadata-only current-registry candidate contains payload writes",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/recovery/original_receipt_tests.rs"]
mod original_receipt_tests;
