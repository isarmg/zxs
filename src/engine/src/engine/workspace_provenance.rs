//! Central publication gates for explicit ordinary Workspace FILE provenance.
//! The scoped producer owns the sealed workspace input, classification and real
//! Version/Placement evidence; these gates supply the actual publication loan.
use super::{OriginalReceiptCursor, PlanningContext, Prepared};
use crate::business::file_draft::origin::{NodeOrigin, Reason};
use crate::journal::Candidate;
use crate::persist::workspace_file_origin as codec;
use crate::snapshot_worktree::ReceiptIndex;
use crate::{Error, OperationResult, OriginalReceiptError, Result, State};
use storage_contract::Principal;

pub(crate) fn relevant(result: &OperationResult) -> bool {
    result.kind == "workspace.commit" || result.result.get("workspace_file_origin").is_some()
}

fn actor(state: &State, receipt: &OperationResult) -> Result<Principal> {
    if state.operations.get(&receipt.operation_id) != Some(receipt) {
        return Err(Error::OriginalReceipt(OriginalReceiptError::Candidate));
    }
    state.operation_principals.get(&receipt.operation_id)
        .ok_or(Error::OriginalReceipt(OriginalReceiptError::Candidate))?
        .parse().map_err(|source| Error::OriginalReceipt(OriginalReceiptError::Principal(source)))
}

pub(super) fn prepared(
    context: &PlanningContext<'_>, next: &State, result: &OperationResult, actual: &[Prepared],
) -> Result<()> {
    if relevant(result) {
        let principal = actor(next, result)?;
        let initial = context.config.generation().is_none() && context.current_namespaces.is_none();
        codec::validate_prepared(
            initial, context.state, next, result, &principal, actual,
            || context.snapshot_receipts().map_err(codec::prior_error),
        )?;
    }
    Ok(())
}

/// A read-only source gate for the *exact* loan. Disjoint snapshot/candidate
/// pointers cannot reuse it to inspect another pending object's material.
pub(crate) struct CandidateDecision<'p, 's> {
    previous: &'p State,
    next: &'s State,
    proposal: &'s Candidate,
}

impl CandidateDecision<'_, '_> {
    pub(crate) fn require_pair(
        &self, previous: &State, next: &State, proposal: &Candidate,
    ) -> Result<()> {
        if !std::ptr::eq(self.previous, previous) || !std::ptr::eq(self.next, next)
            || !std::ptr::eq(self.proposal, proposal)
        {
            return Err(Error::OriginalReceipt(OriginalReceiptError::Candidate));
        }
        Ok(())
    }
}

pub(crate) fn prefix_index<'p>(
    previous: &'p State, originals: &'p OriginalReceiptCursor,
) -> Result<ReceiptIndex<'p>> {
    ReceiptIndex::capture_with_originals(previous, originals).map_err(codec::prior_error)
}

/// No mutable spool, later visible receipt or current namespace lookup is allowed
/// to stand in for the two original graph sides and Candidate's actual versions.
/// A reserved Worktree child can have precisely the shape of an ordinary FILE
/// write. Its complete original prior reservation index is borrowed lazily.
pub(crate) fn candidate<'p, 's>(
    previous: &'p State, next: &'s State, proposal: &'s Candidate,
    prior: impl FnOnce() -> Result<ReceiptIndex<'p>>,
) -> Result<CandidateDecision<'p, 's>> {
    let receipt = next.operations.get(&proposal.operation_id)
        .ok_or(Error::OriginalReceipt(OriginalReceiptError::Candidate))?;
    if relevant(receipt) {
        let principal = actor(next, receipt)?;
        codec::validate(proposal, previous, next, receipt, &principal, prior)?;
    }
    Ok(CandidateDecision { previous, next, proposal })
}

pub(crate) fn from_context<'p, 's>(
    context: &PlanningContext<'p>, next: &'s State, proposal: &'s Candidate,
) -> Result<CandidateDecision<'p, 's>> {
    candidate(context.state, next, proposal, || context.snapshot_receipts().map_err(codec::prior_error))
}

/// Standalone State decoding can only check retained shape. The result remains
/// unauthorized until the original adjacent-candidate/publication proof above.
pub(crate) fn retained(state: &State, result: &OperationResult) -> Result<()> {
    if relevant(result) {
        codec::validate_retained(state, result, &actor(state, result)?)?;
    }
    Ok(())
}

/// Only invoked on the separately verified original snapshot/actor after its
/// actual Candidate and completed publication have been admitted. The public
/// result is still the original Workspace receipt, never a new draft decision.
pub(crate) fn original(
    state: &State, result: &OperationResult, actor: &Principal,
) -> Result<Option<NodeOrigin>> {
    if !relevant(result) {
        return Ok(None);
    }
    Ok(codec::validate_retained(state, result, actor)?.map(|v| NodeOrigin {
        reason: Reason::ManualFile {}, target: v.target, owner: v.owner, ancestors: v.ancestors,
        node: v.node, operation: v.operation, sequence: v.sequence, published_at: v.published_at,
    }))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/workspace_provenance_tests.rs"]
mod tests;
