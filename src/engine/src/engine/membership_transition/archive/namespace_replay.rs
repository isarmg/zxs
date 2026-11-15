//! Namespace reconstruction consumes the same independently verified immutable
//! control archive as generation recovery. No historical disk is reopened.
use super::*;
use std::sync::Arc;

/// Created only after the complete frozen journals, fixed-member seals, source
/// snapshots and prescribed first publications have passed archive verification.
pub(in crate::engine::membership_transition) struct VerifiedArchivePublication {
    candidate: Candidate,
    original: Arc<State>,
    recovered_result: Option<OperationResult>,
    activation: Option<PortableAuthority>,
}
impl VerifiedArchivePublication {
    pub(in crate::engine::membership_transition) fn candidate(&self) -> &Candidate {
        &self.candidate
    }
    pub(in crate::engine::membership_transition) fn original_state(&self) -> &Arc<State> {
        &self.original
    }

    // The one successful business transition consumes original_state first.
    // Only its next visible prefix includes the independently verified recovery
    // outcome. No original snapshot or receipt is modified in place.
    pub(in crate::engine::membership_transition) fn visible_state(&self) -> Arc<State> {
        let Some(result) = &self.recovered_result else {
            return Arc::clone(&self.original);
        };
        let mut visible = self.original.as_ref().clone();
        visible.operations.insert(self.candidate.operation_id, result.clone());
        Arc::new(visible)
    }
    pub(in crate::engine::membership_transition) fn activation(
        &self,
    ) -> Option<&PortableAuthority> {
        self.activation.as_ref()
    }
}

impl PendingControlArchive {
    pub(in crate::engine::membership_transition) fn replay_publications(
        &self,
        mut visit: impl FnMut(&VerifiedArchivePublication) -> Result<()>,
    ) -> Result<()> {
        // Re-read all evidence, including each original member's own immutable
        // journal. A cached inspection or serialized namespace is not authority.
        verify_root(&self.store, &self.root, &self.root.transition_hash)?;
        let intent: TransitionIntent = json_codec::from_value(
            self.store
                .load(INTENT_KIND, &self.root.transition_hash)?
                .into_value(),
        )?;
        let mut previous_sequence = None;
        for (epoch, intent) in archive_epochs(&self.store, &self.root, &intent)? {
            let authority = intent
                .origin
                .predecessor()
                .map(|hash| PortableAuthority::load(hash, &mut &self.store))
                .transpose()?;
            let mut ordered = BTreeMap::new();
            for (hash, seal_hash) in archive_seals(&self.store, &epoch)? {
                let candidate: Candidate =
                    json_codec::from_value(self.store.load("candidate", &hash)?.into_value())?;
                if ordered
                    .insert(candidate.sequence, (candidate, seal_hash))
                    .is_some()
                {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "archive namespace replay repeats a sequence",
                    ));
                }
            }
            for (sequence, (candidate, seal_hash)) in ordered {
                if previous_sequence.is_some_and(|previous| sequence <= previous) {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "archive namespace epochs overlap",
                    ));
                }
                let seal: PublicationSeal =
                    json_codec::from_value(self.store.load(SEAL_KIND, &seal_hash)?.into_value())?;
                let publication: Publication = json_codec::from_value(
                    self.store
                        .load("publication", &seal.publication_hash)?
                        .into_value(),
                )?;
                verify_publication_seal(&candidate, &publication, &seal)?;
                let snapshot: Snapshot = json_codec::from_value(
                    self.store
                        .load("snapshot", &candidate.snapshot_hash)?
                        .into_value(),
                )?;
                let original =
                    resolve_admitted_snapshot(snapshot, |kind, hash| self.store.load(kind, hash))?
                        .into_state();
                let recovered_result = publication.recovery.is_some().then_some(publication.result);
                let activation = match &authority {
                    Some(authority)
                        if sequence
                            == authority
                                .intent()?
                                .fence_sequence
                                .checked_add(1)
                                .ok_or_else(|| {
                                    Error::new(
                                        ErrorCode::ResourceLimit,
                                        "activation sequence exhausted",
                                    )
                                })? =>
                    {
                        Some(authority.clone())
                    }
                    _ => None,
                };
                visit(&VerifiedArchivePublication {
                    candidate,
                    original: Arc::new(original),
                    recovered_result,
                    activation,
                })?;
                previous_sequence = Some(sequence);
            }
        }
        Ok(())
    }
}

pub(in crate::engine::membership_transition) fn replay_installed_publications(
    root: DiskRoot,
    expected: &str,
    transition: &str,
    visit: impl FnMut(&VerifiedArchivePublication) -> Result<()>,
) -> Result<()> {
    PendingControlArchive::from_installed(root, expected, transition)?.replay_publications(visit)
}
