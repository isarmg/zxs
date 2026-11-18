//! Namespace reconstruction consumes the same independently verified immutable
//! control archive as generation recovery. No historical disk is reopened.
use super::*;
use std::sync::Arc;

/// Created only after the complete frozen journals, fixed-member seals, source
/// snapshots and prescribed first publications have passed archive verification.
pub(crate) struct VerifiedArchivePublication {
    candidate: Candidate,
    original: Arc<State>,
    publication: Publication,
    seal: PublicationSeal,
    activation: Option<PortableAuthority>,
}
impl VerifiedArchivePublication {
    pub(crate) fn candidate(&self) -> &Candidate {
        &self.candidate
    }
    pub(crate) fn original_state(&self) -> &Arc<State> {
        &self.original
    }

    pub(crate) fn publication(&self) -> &Publication {
        &self.publication
    }

    pub(crate) fn seal(&self) -> &PublicationSeal {
        &self.seal
    }

    // The one successful business transition consumes original_state first.
    // Only its next visible prefix includes the independently verified recovery
    // outcome. No original snapshot or receipt is modified in place.
    pub(crate) fn visible_state(&self) -> Arc<State> {
        if self.publication.recovery.is_none() {
            return Arc::clone(&self.original);
        }
        let mut visible = self.original.as_ref().clone();
        visible.operations.insert(
            self.candidate.operation_id,
            self.publication.result.clone(),
        );
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
        // A loan of the actual prior published outcome, never a replay against
        // today's namespace or a newly synthesized original Workspace.
        let mut original_prefix=crate::engine::recovery::ArchiveWorkspacePrefix::default();
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
                let slot=VerifiedArchivePublication {candidate,original:Arc::new(original),publication,seal,activation};
                let prior=original_prefix.predecessor(&slot)?;
                let result=slot.original.operations.get(&slot.candidate.operation_id)
                    .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
                if crate::engine::workspace_provenance::relevant(result) || crate::persist::file_adoption::is_kind(&result.kind) {
                    let previous=prior.ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix))?;
                    crate::engine::workspace_provenance::candidate(previous,&slot.original,&slot.candidate,
                        ||original_prefix.index(previous))?;
                    crate::persist::file_adoption::transition(previous,&slot.original,result,!slot.candidate.versions.is_empty())?;
                }
                original_prefix.advance(&slot)?;
                visit(&slot)?;
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

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/archive_original_receipt_tests.rs"]
mod original_receipt_tests;
