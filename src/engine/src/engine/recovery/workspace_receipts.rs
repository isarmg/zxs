//! Verify published receipts before workspace admission or recovery writes.
use super::*;
use std::sync::Arc;

pub(super) struct PreparedCandidate {
    pub candidate: Candidate,
    pub publication: Option<Publication>,
    pub barrier_failure: Option<Error>,
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
        // Main's operation and operation_principals maps remain authoritative.
        // Only the latest fully published State is needed for cold admission;
        // no alternate receipt index or persistent representation is introduced.
        let state = if let Some(sequence) = published {
            let bound = &prepared.candidates[&sequence];
            let mut state = verified_snapshot_at(
                &self.disk_resources,
                &prepared.online,
                &bound.candidate.snapshot_hash,
            )?
            .into_state();
            let publication = bound
                .publication
                .as_ref()
                .expect("invariant: verified publication");
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
            if publication.recovery.is_some() {
                state
                    .operations
                    .insert(bound.candidate.operation_id, publication.result.clone());
            }
            Arc::new(state)
        } else {
            Arc::new(State {
                topology_generation: 1,
                ..State::default()
            })
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
        if candidate.sequence != sequence
            || candidate.pool_id != self.config.pool_id
            || &candidate.previous_snapshot != previous
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "candidate does not extend the verified publication prefix",
            ));
        }
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
