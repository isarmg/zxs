//! Exact original snapshot evidence, before any recovered outcome is overlaid.
use super::verified_snapshot_at;
use crate::{
    DiskConfig, Error, ErrorCode, OperationId, OperationResult, Result, State,
    disk::DiskResources,
    history_package::{AdmittedState, SharedAdmittedState},
    journal::{Candidate, Publication, PublicationSeal, verify_publication_seal},
    OriginalReceiptError,
};
use std::sync::Arc;
use storage_contract::{CommitState, Principal};

/// Constructed only after the referenced snapshot and its complete closure have
/// passed the ordinary disk authentication and RawState admission path.
/// No constructor accepts a visible Engine State or a caller-supplied proof.
pub(crate) struct AdmittedSnapshot {
    snapshot_hash: String,
    original: AdmittedState,
}

impl AdmittedSnapshot {
    pub(crate) fn for_candidate(&self, candidate: &Candidate) -> Result<&AdmittedState> {
        if self.snapshot_hash != candidate.snapshot_hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "admitted snapshot differs from candidate root",
            ));
        }
        Ok(&self.original)
    }

    /// A finalized object may precede the complete D3 Publication barrier.
    /// Validate its exact original/outcome relation before recovery writes, but
    /// do not issue an original projection or install any visible source yet.
    pub(crate) fn require_finalized_outcome(
        &self, candidate: &Candidate, publication: &Publication,
    ) -> Result<()> {
        crate::journal::verify_finalized_publication(candidate, publication)?;
        let original = self.for_candidate(candidate)?.state();
        original_outcome(original, candidate, publication).map(|_| ())
    }

    pub(crate) fn into_shared(self) -> SharedAdmittedSnapshot {
        SharedAdmittedSnapshot {
            snapshot_hash: self.snapshot_hash,
            original: self.original.into_shared(),
        }
    }

    /// End the original disk-snapshot proof before producing visible recovery
    /// state. The returned State cannot stand in for the original snapshot when
    /// a recovery publication replaces its operation outcome.
    pub(crate) fn into_visible_state(
        self,
        operation: OperationId,
        recovered_result: Option<OperationResult>,
    ) -> State {
        let mut state = self.original.into_state();
        if let Some(result) = recovered_result {
            state.operations.insert(operation, result);
        }
        state
    }
}

/// A read-only projection from one admitted original and its verified outcome.
/// It is not a publication barrier, format admission, State or writer proof.
/// In particular, callers cannot recover an original by deleting visible fields.
#[derive(PartialEq)]
pub(crate) struct OriginalReceiptProjection {
    original: OperationResult,
    visible: OperationResult,
    principal: Principal,
}

impl OriginalReceiptProjection {
    /// The archive producer has already authenticated every frozen member
    /// stream, this Candidate's original closure and its actual publication.
    /// Reuse the same overlay kernel; visible words never supply an original.
    pub(crate) fn from_archive(
        source: &crate::engine::membership_transition::VerifiedArchivePublication,
        visible: &State,
    ) -> Result<Self> {
        project_original_receipt(
            source.original_state(), source.candidate(), source.publication(),
            source.seal(), visible,
        )
    }

    /// A search key only from the authentic originally published receipt, never
    /// from a visible recovery overlay or the current FILE controller.
    pub(crate) fn automatic_key(&self) -> Result<Option<storage_contract::automatic_observation::NodeKey>> {
        use crate::business::file_draft::origin::Reason;
        if !matches!(self.original.kind.as_str(), "file.draft.commit" | crate::persist::file_draft::unborn_first::KIND) {
            return Ok(None);
        }
        Ok(crate::persist::file_draft::observed_node(&self.original)?.and_then(|node| {
            matches!(node.reason, Reason::AutomaticSave { .. }).then_some(
                storage_contract::automatic_observation::NodeKey { file: node.target.file, node: node.node })
        }))
    }

    pub(crate) fn operation(&self) -> OperationId {
        self.original.operation_id
    }

    pub(crate) fn sequence(&self) -> storage_contract::Sequence {
        storage_contract::Sequence::new(self.original.sequence)
    }

    /// A later prefix may contain more operations, but this permanent result
    /// and its original actor must still be exactly the observed pair.
    pub(crate) fn in_prefix<'a>(
        &'a self, visible: &State,
    ) -> Result<(&'a OperationResult, &'a Principal)> {
        if visible.operations.get(&self.operation()) != Some(&self.visible)
            || visible.operation_principals.get(&self.operation()).map(String::as_str)
                != Some(self.principal.as_str())
            || visible.sequence < self.visible.sequence
        {
            return Err(Error::OriginalReceipt(OriginalReceiptError::Prefix));
        }
        Ok((&self.original, &self.principal))
    }
}

fn project_original_receipt(
    original: &State,
    candidate: &Candidate,
    publication: &Publication,
    seal: &PublicationSeal,
    visible: &State,
) -> Result<OriginalReceiptProjection> {
    // Preserve the existing seal/candidate/recovery-proof failure precedence.
    // The scanner separately observes this same barrier on its held journals.
    verify_publication_seal(candidate, publication, seal)?;
    let (receipt, principal, expected_result) = original_outcome(original, candidate, publication)?;
    let mut expected_state = original.clone();
    expected_state.operations.insert(candidate.operation_id, expected_result.clone());
    // Compare the entire relation, including untouched operations, actor map,
    // namespace, payload and working rows, not just recovery labels/digests.
    if expected_state != *visible {
        return Err(Error::OriginalReceipt(OriginalReceiptError::Visible));
    }
    Ok(OriginalReceiptProjection {
        original: receipt.clone(), visible: expected_result, principal,
    })
}

/// One canonical original/outcome relation for both finalized-only checks and
/// completed projections. Only the latter separately proves the full barrier.
fn original_outcome<'a>(
    original: &'a State, candidate: &Candidate, publication: &Publication,
) -> Result<(&'a OperationResult, Principal, OperationResult)> {
    let receipt = original.operations.get(&candidate.operation_id)
        .ok_or(Error::OriginalReceipt(OriginalReceiptError::Candidate))?;
    if original.sequence != candidate.sequence
        || receipt.operation_id != candidate.operation_id
        || receipt.sequence != candidate.sequence
        || receipt.request_digest != candidate.request_digest
        || receipt.commit_state != CommitState::Published
    {
        return Err(Error::OriginalReceipt(OriginalReceiptError::Candidate));
    }
    let actor = original.operation_principals.get(&candidate.operation_id)
        .ok_or(Error::OriginalReceipt(OriginalReceiptError::MissingPrincipal))?;
    let principal = actor.parse().map_err(|source| {
        Error::OriginalReceipt(OriginalReceiptError::Principal(source))
    })?;
    let expected_result = match &publication.recovery {
        Some(proof) => crate::persist::degraded_result::apply(receipt.clone(), &proof.plan),
        None => receipt.clone(),
    };
    if expected_result != publication.result {
        return Err(Error::OriginalReceipt(OriginalReceiptError::Visible));
    }
    Ok((receipt, principal, expected_result))
}

/// The short workspace-admission loan retains the same snapshot identity.
pub(crate) struct SharedAdmittedSnapshot {
    snapshot_hash: String,
    original: SharedAdmittedState,
}

impl SharedAdmittedSnapshot {
    pub(crate) fn project_original_receipt(
        &self,
        candidate: &Candidate,
        publication: &Publication,
        seal: &PublicationSeal,
        visible: &State,
    ) -> Result<OriginalReceiptProjection> {
        // Reborrow the same admitted original; never admit the visible State.
        let original = self.original.share_state();
        if self.snapshot_hash != candidate.snapshot_hash {
            return Err(Error::OriginalReceipt(OriginalReceiptError::Candidate));
        }
        project_original_receipt(&original, candidate, publication, seal, visible)
    }

    pub(crate) fn share_state(&self) -> Arc<State> {
        self.original.share_state()
    }

    pub(crate) fn into_admitted(self) -> AdmittedSnapshot {
        AdmittedSnapshot {
            snapshot_hash: self.snapshot_hash,
            original: self.original.into_admitted(),
        }
    }
}

pub(crate) fn verified_recovery_snapshot_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    hash: &str,
) -> Result<AdmittedSnapshot> {
    let original = verified_snapshot_at(resources, members, hash)?;
    Ok(AdmittedSnapshot {
        snapshot_hash: hash.to_owned(),
        original,
    })
}
