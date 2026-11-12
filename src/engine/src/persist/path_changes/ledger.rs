//! Validate immutable audit envelopes at snapshot and lifecycle-read boundaries.
//! This does not replay past namespace transitions from a final State. Current
//! publication/recovery still proves each actual before/after transition.
use crate::{Result, State};
use crate::business::{history_copy, object_move};
use crate::persist::path_changes::{
    PathChanged, batch_transfers, invalid, is_kind, read, validate_receipt_facts,
};
use crate::state_projection::lifecycles::{DeletionOwner, history_copy_owner};
use storage_contract::Principal;

pub(crate) fn admit(state: &State) -> Result<()> {
    visit(state, || Ok(()), |_| Ok(()))
}

pub(crate) fn visit(
    state: &State,
    mut checkpoint: impl FnMut() -> Result<()>,
    mut path: impl FnMut(PathChanged) -> Result<()>,
) -> Result<()> {
    for (id, receipt) in state.operations.iter() {
        checkpoint()?;
        let copies = history_copy_owner(receipt);
        if !is_kind(&receipt.kind) && matches!(copies, DeletionOwner::Other) {
            continue;
        }
        if receipt.operation_id != *id
            || receipt.sequence == 0
            || receipt.sequence > state.sequence
        {
            return Err(invalid());
        }
        let principal = state.operation_principals.get(id).ok_or_else(invalid)?;
        let principal = Principal::new(principal.clone())
            .map_err(|source| crate::Error::PathChange(crate::PathChangeError::Principal(source)))?;
        let facts = if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
            // The move decoder admits each required retained-copy envelope too.
            let movement = object_move::read_result(receipt)?;
            if movement.movement.principal != principal.as_str() {
                return Err(invalid());
            }
            validate_receipt_facts(receipt, &movement.path_changes)?;
            movement.path_changes
        } else {
            match copies {
                DeletionOwner::File(file) => {
                    history_copy::retained::read(receipt, file)?;
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
            path(fact)?;
        }
    }
    Ok(())
}
