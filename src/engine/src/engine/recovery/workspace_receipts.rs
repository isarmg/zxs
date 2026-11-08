//! Verify published receipts before workspace admission or recovery writes.
use super::*;
use crate::engine::disk_registry::current::CurrentNamespaces;
use std::sync::Arc;

pub(super) struct PreparedCandidate {
    pub candidate: Candidate,
    pub publication: Option<Publication>,
    pub barrier_failure: Option<Error>,
    // Only this exact candidate.snapshot_hash, consumed once under the same
    // admitted roots/locks. Never an all-history State cache.
    pub snapshot: Option<crate::publication_evidence::SharedAdmittedSnapshot>,
    // A pure next view for this preflight slot, never an Engine installation.
    next_namespaces: Option<CurrentNamespaces>,
}

pub(super) struct ReceiptPrefix<'a> {
    pub snapshot: &'a Option<String>,
    pub state: &'a State,
    pub namespaces: Option<&'a CurrentNamespaces>,
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
        for (sequence, candidates) in &prepared.records {
            let mut active = candidates.iter().filter(|(_, receipts)| {
                receipts.iter().any(|receipt| {
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
                },
            )?;
            let barrier = match &candidate.publication {
                Some(publication) => published_barrier(
                    self,
                    &prepared.online,
                    receipts,
                    &candidate.candidate,
                    publication,
                ),
                None => {
                    prepared.candidates.insert(*sequence, candidate);
                    break;
                }
            };
            let seal = match barrier {
                Ok(seal) => seal,
                Err(source) => {
                    // This slot may still be repairable by apply. It cannot grant
                    // cold admission. Keep the concrete failure for diagnostics.
                    candidate.barrier_failure = Some(source);
                    prepared.candidates.insert(*sequence, candidate);
                    break;
                }
            };
            if candidate.candidate.registry().is_some() {
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
                let admitted = candidate
                    .snapshot
                    .take()
                    .expect("invariant: registry preflight authenticated its snapshot")
                    .into_admitted();
                let original = admitted.for_candidate(&candidate.candidate)?;
                namespaces = Some(match candidate.next_namespaces.take() {
                    Some(view) => view,
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
                candidate.snapshot = Some(shared);
            }
            previous = Some(candidate.candidate.snapshot_hash.clone());
            published = Some(*sequence);
            // Keep only the latest proven slot, plus at most the first pending
            // slot. Earlier candidates are streamed again for historical repair.
            prepared.candidates.clear();
            prepared.candidates.insert(*sequence, candidate);
        }
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
        let original = std::mem::replace(&mut self.state, state);
        let admission = self.seed_workspace_recovery();
        self.state = original;
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
        // Even ACCEPT-only discovery may open a data-only local proof. Prove
        // this slot against its exact admitted predecessor before that search;
        // receipt labels alone cannot grant permission to inspect payloads.
        let mut next_namespaces = None;
        let snapshot = if candidate.registry().is_some() {
            let admitted = verified_recovery_snapshot_at(
                &self.disk_resources,
                online,
                &candidate.snapshot_hash,
            )?;
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
            Some(admitted.into_shared())
        } else {
            None
        };
        let publication = match receipts
            .iter()
            .find(|receipt| receipt.record.record_type == RecordType::Publication)
        {
            Some(receipt) => Some(verified_object_at(
                &self.disk_resources,
                online,
                "publication",
                &receipt.record.object_hash,
            )?),
            None => crate::degraded_recovery::discover_publication(self, &candidate, hash)?,
        };
        if publication
            .as_ref()
            .is_some_and(|publication| publication.recovery.is_some())
            && let Some(snapshot) = &snapshot
        {
            CurrentNamespaces::reject_degraded_backup(&candidate, &snapshot.share_state())?;
            CurrentNamespaces::reject_degraded_history_copy(&candidate, &snapshot.share_state())?;
        }
        Ok(PreparedCandidate {
            candidate,
            publication,
            barrier_failure: None,
            snapshot,
            next_namespaces,
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

    pub(crate) fn verify_current_recovery_candidate(
        &self,
        candidate: &Candidate,
        next: &State,
    ) -> Result<()> {
        if let Some(view) = self.admitted_current_namespaces()? {
            verify_current_candidate_transition(view, &self.state, next, candidate)?;
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
        "plan.cancel" | "branch.archive" | "merge-link.create" | "file.tags.update"
    ) && !candidate.versions.is_empty()
    {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "metadata-only current-registry candidate contains payload writes",
        ));
    }
    Ok(())
}
