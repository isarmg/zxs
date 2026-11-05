//! Verify published receipts before workspace admission or recovery writes.
use super::*;
use std::sync::Arc;

pub(super) struct PreparedCandidate {
    pub candidate: Candidate,
    pub publication: Option<Publication>,
    pub barrier_failure: Option<Error>,
    // Only this exact candidate.snapshot_hash, consumed once under the same
    // admitted roots/locks. Never an all-history State cache.
    pub snapshot: Option<crate::publication_evidence::SharedAdmittedSnapshot>,
}

impl Engine {
    pub(in crate::engine) fn preflight_workspace_receipts(
        &mut self,
        prepared: &mut PreparedRecovery,
    ) -> Result<()> {
        let mut previous = self.snapshot.clone();
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
                &previous,
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
            if let Err(source) = barrier {
                // This slot may still be repairable by apply. It cannot grant
                // cold admission. Keep the concrete failure for diagnostics.
                candidate.barrier_failure = Some(source);
                prepared.candidates.insert(*sequence, candidate);
                break;
            }
            previous = Some(candidate.candidate.snapshot_hash.clone());
            published = Some(*sequence);
            // Keep only the latest proven slot, plus at most the first pending
            // slot. Earlier candidates are streamed again for historical repair.
            prepared.candidates.clear();
            prepared.candidates.insert(*sequence, candidate);
        }
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
        previous: &Option<String>,
    ) -> Result<PreparedCandidate> {
        let candidate: Candidate =
            verified_object_at(&self.disk_resources, online, "candidate", hash)?;
        self.verify_candidate_membership(&candidate)?;
        if let Some(generation) = self.config.generation()
            && previous == &generation.snapshot()
        {
            generation.verify_activation_candidate(&candidate)?;
        }
        if candidate.sequence != sequence
            || candidate.pool_id != self.config.pool_id
            || &candidate.previous_snapshot != previous
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "candidate does not extend the verified publication prefix",
            ));
        }
        // Publication discovery can read local payload proofs even for an
        // ACCEPT-only candidate. Authenticate the original snapshot receipt
        // before letting a current-registry envelope drive any such I/O.
        let snapshot = if candidate.registry().is_some() && !candidate.versions.is_empty() {
            let admitted = verified_recovery_snapshot_at(
                &self.disk_resources,
                online,
                &candidate.snapshot_hash,
            )?;
            verify_current_candidate_receipt(
                &candidate,
                admitted.for_candidate(&candidate)?.state(),
            )?;
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
        Ok(PreparedCandidate {
            candidate,
            publication,
            barrier_failure: None,
            snapshot,
        })
    }
}

fn published_barrier(
    engine: &Engine,
    online: &[DiskConfig],
    receipts: &[Receipt],
    candidate: &Candidate,
    publication: &Publication,
) -> Result<()> {
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
        verify_publication_seal(
            candidate,
            publication,
            &PublicationSeal {
                candidate_hash: publication.candidate_hash.clone(),
                publication_hash,
                receipts,
            },
        )
    } else {
        verified_publication_seal_at(&engine.disk_resources, online, candidate, publication)
            .map(|_| ())
    }
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
    if matches!(result.kind.as_str(), "plan.cancel" | "branch.archive")
        && !candidate.versions.is_empty()
    {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "metadata-only current-registry candidate contains payload writes",
        ));
    }
    Ok(())
}
