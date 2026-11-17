//! Validate immutable audit envelopes at snapshot and lifecycle-read boundaries.
//! This does not replay past namespace transitions from a final State. Current
//! publication/recovery still proves each actual before/after transition.
use crate::{OperationResult, Result, State};
use crate::business::{history_copy, object_move};
use crate::persist::path_changes::{
    PathChanged, batch_transfers, invalid, is_kind, read, validate_receipt_facts,
};
use crate::persist::path_changes::references::References;
use crate::state_projection::lifecycles::{DeletionOwner, history_copy_owner};
use storage_contract::{OperationId, Principal, Sequence};

pub(crate) fn admit(state: &State) -> Result<()> {
    visit(state, || Ok(()), |_| Ok(()))
}

/// These are all carriers, including malformed receipts of the expected kinds.
/// Ownership fields must never decide whether their envelope is read.
fn carries_ledger(kind: &str) -> bool {
    is_kind(kind)
        || kind == crate::history_package::path_audit::RESTORE_KIND
        || history_copy::is_kind(kind)
        || kind == crate::business::FILE_HISTORY_MOVE_KIND
}

/// A temporary, borrowed selection for this exact read. Only an exhaustive pass
/// over the original ordered map can build the sparse variant. Cache overflow
/// or a dense ledger falls back to the map, without rejecting a valid audit.
pub(crate) struct LedgerRead<'a> {
    state: &'a State,
    sparse: Option<Vec<(&'a OperationId, &'a OperationResult)>>,
}

impl<'a> LedgerRead<'a> {
    pub(crate) fn all(state: &'a State) -> Self {
        Self { state, sparse: None }
    }

    pub(crate) fn with_memberships(
        state: &'a State,
        mut checkpoint: impl FnMut() -> Result<()>,
        mut member: impl FnMut(OperationId, &OperationResult) -> Result<()>,
    ) -> Result<Self> {
        let maximum = state.operations.len().div_ceil(4)
            .min(storage_contract::file_lifecycle::MAX_AUDIT_EVENTS);
        let mut sparse = Some(Vec::new());
        for (id, receipt) in state.operations.iter() {
            checkpoint()?;
            member(*id, receipt)?;
            if let Some(rows) = &mut sparse
                && carries_ledger(&receipt.kind)
            {
                if rows.len() == maximum {
                    sparse = None;
                } else {
                    rows.push((id, receipt));
                }
            }
        }
        Ok(Self { state, sparse })
    }

    pub(crate) fn visit(
        &self,
        checkpoint: impl FnMut() -> Result<()>,
        path: impl FnMut(PathChanged) -> Result<()>,
    ) -> Result<()> {
        match &self.sparse {
            Some(rows) => visit_rows(self.state, rows.iter().copied(), checkpoint, path),
            None => visit(self.state, checkpoint, path),
        }
    }
}

pub(crate) fn visit(
    state: &State,
    checkpoint: impl FnMut() -> Result<()>,
    path: impl FnMut(PathChanged) -> Result<()>,
) -> Result<()> {
    visit_rows(state, state.operations.iter(), checkpoint, path)
}

fn visit_rows<'a>(
    state: &State,
    rows: impl Iterator<Item = (&'a OperationId, &'a OperationResult)>,
    mut checkpoint: impl FnMut() -> Result<()>,
    mut path: impl FnMut(PathChanged) -> Result<()>,
) -> Result<()> {
    let mut references = References::new(state);
    let mut collisions = crate::history_package::path_audit::CollisionIndex::new(state);
    for (id, receipt) in rows {
        checkpoint()?;
        if !carries_ledger(&receipt.kind) {
            continue;
        }
        let imported = receipt.kind == crate::history_package::path_audit::RESTORE_KIND;
        if receipt.operation_id != *id
            || receipt.sequence == 0
            || receipt.sequence > state.sequence
        {
            return Err(invalid());
        }
        let principal = state.operation_principals.get(id).ok_or_else(invalid)?;
        let principal = Principal::new(principal.clone().into())
            .map_err(|source| crate::Error::PathChange(crate::PathChangeError::Principal(source)))?;
        if imported {
            let sources = crate::history_package::path_audit::restored(receipt)?;
            collisions.admit(&sources)?;
            for source in sources.values() {
                for fact in &source.projections {
                    references.check(
                        fact.file_id, fact.history_tree_id, fact.branch_id,
                        [fact.before_version_id, fact.after_version_id],
                        Sequence::new(receipt.sequence), &mut checkpoint,
                    )?;
                }
            }
            // Imported provenance is permanent but not a new native path event.
            continue;
        }
        let facts = if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
            // The move decoder admits each required retained-copy envelope too.
            let movement = object_move::read_result(receipt)?;
            if movement.movement.principal != principal.as_str() {
                return Err(invalid());
            }
            validate_receipt_facts(receipt, &movement.path_changes)?;
            for copied in &movement.retained_lifecycle {
                collisions.admit(&copied.path_audit)?;
                for fact in &copied.path_changes {
                    references.check(
                        fact.file_id,
                        fact.history_tree_id,
                        fact.branch_id,
                        [fact.before_version_id, fact.after_version_id],
                        Sequence::new(receipt.sequence),
                        &mut checkpoint,
                    )?;
                }
            }
            movement.path_changes
        } else {
            match history_copy_owner(receipt) {
                DeletionOwner::File(file) => {
                    let copied = history_copy::retained::read(receipt, file)?;
                    collisions.admit(&copied.path_audit)?;
                    for fact in &copied.path_changes {
                        references.check(
                            fact.file_id,
                            fact.history_tree_id,
                            fact.branch_id,
                            [fact.before_version_id, fact.after_version_id],
                            Sequence::new(receipt.sequence),
                            &mut checkpoint,
                        )?;
                    }
                }
                DeletionOwner::Other => {}
                DeletionOwner::Files(_) | DeletionOwner::Malformed => return Err(invalid()),
            }
            if is_kind(&receipt.kind) {
                batch_transfers(receipt, &principal)?;
                read(receipt)?
            } else {
                Vec::new()
            }
        };
        for fact in facts {
            checkpoint()?;
            if fact.author != principal {
                return Err(invalid());
            }
            references.check(
                fact.file_id,
                fact.history_tree_id,
                fact.branch_id,
                [fact.before_version_id, fact.after_version_id],
                Sequence::new(receipt.sequence),
                &mut checkpoint,
            )?;
            path(fact)?;
        }
    }
    Ok(())
}
