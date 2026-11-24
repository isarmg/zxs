//! Query one operation under the same container gate as publication and recycle.
use super::{Mutation, Receipt, RecycleReceipt, State, Store, invalid, read_state};
use std::io;

/// A request has exactly one selected disposition. Unavailable/corrupt media
/// are errors, never evidence that a request was absent or should be retried.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum OperationReceipt {
    Mutation(Receipt),
    Recycle(RecycleReceipt),
}

impl Store {
    pub(crate) fn operation_receipt(&self, op: Mutation) -> io::Result<Option<OperationReceipt>> {
        let object = self.object(op.container)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        self.receipt_under_gate(op, &mut memory)
    }

    pub(super) fn receipt_under_gate(
        &self,
        op: Mutation,
        memory: &mut Option<State>,
    ) -> io::Result<Option<OperationReceipt>> {
        let location = self.recycled(op.container)?;
        let result = if let Some((selected, at)) = location.as_ref() {
            // SYSTEM selection alone cannot prove that this operation was
            // absent from the chosen ledger. Never confuse corrupt R with 404.
            let state = self.name_observed(read_state(at, op.container))?;
            self.name_observed(require_archived_ledger(&state, selected))?;
            if selected.mutation == op {
                Some(OperationReceipt::Recycle(*selected))
            } else {
                state
                    .receipts
                    .get(&op.request)
                    .cloned()
                    .map(OperationReceipt::Mutation)
            }
        } else if let Some(volume) = self.routed(op.container)? {
            self.name_observed(volume.load(op.container, memory))?
                .receipts
                .get(&op.request)
                .cloned()
                .map(OperationReceipt::Mutation)
        } else {
            None
        };
        // The gate excludes another ZXS writer, not a mount going away
        // during a long ledger read. Requalify the *same* location before
        // exposing even a negative answer; do not choose another ledger.
        match (location, self.recycled(op.container)?) {
            (None, None) => {}
            (Some((prior, at)), Some((now, named)))
                if prior == now
                    && at.entity == named.entity
                    && self.name_observed(at.same_mount(&named))? =>
            {
                self.name_observed(at.require_named())?;
            }
            _ => {
                return self.name_observed(Err(invalid(
                    "container receipt location changed during lookup",
                )));
            }
        }
        Ok(result)
    }
}

pub(super) fn require_archived_ledger(state: &State, selected: &RecycleReceipt) -> io::Result<()> {
    if state.sequence != selected.original_sequence
        || state.name.is_none()
        || state.receipts.contains_key(&selected.mutation.request)
    {
        return Err(invalid(
            "archived ledger disagrees with its selected recycle receipt",
        ));
    }
    Ok(())
}
