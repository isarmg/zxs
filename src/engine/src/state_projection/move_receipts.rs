//! Lazily decode each movement once for one immutable State projection.
//! These derived rows expire with the transition, never with a published owner index.
use crate::{
    Error, ErrorCode, FileDeletionNode, OperationId, OperationResult, Result, State,
    business::{history_copy::retained::CopiedLifecycle, object_move},
    file_deletion::retained::DeletionAction,
};
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

pub(super) struct MoveReceipts<'a> {
    state: &'a State,
    decoded: RefCell<BTreeMap<OperationId, Arc<MoveReceipt>>>,
}

impl<'a> MoveReceipts<'a> {
    pub(super) fn new(state: &'a State) -> Self {
        Self {
            state,
            decoded: RefCell::new(BTreeMap::new()),
        }
    }

    pub(super) fn get(&self, id: OperationId) -> Result<Arc<MoveReceipt>> {
        use std::collections::btree_map::Entry;
        let mut receipts = self.decoded.borrow_mut();
        let receipt = match receipts.entry(id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let result = MoveReceipt::read(&self.state.operations[&id])?;
                entry.insert(Arc::new(result))
            }
        };
        Ok(Arc::clone(receipt))
    }
}

/// Bounded, decoded lifecycle facts with one file index. This is not a proof of
/// publication, current namespace authority, or on-disk payload integrity.
pub(crate) struct MoveReceipt {
    deletions: BTreeMap<String, (DeletionAction, FileDeletionNode)>,
    retained: BTreeMap<String, CopiedLifecycle>,
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "object movement lifecycle differs from its original members",
    )
}

impl MoveReceipt {
    fn read(receipt: &OperationResult) -> Result<Self> {
        let result = object_move::read_result(receipt)?;
        let mut deletions = BTreeMap::new();
        for (action, nodes) in [
            (DeletionAction::Delete, result.source_deletions),
            (DeletionAction::Delete, result.displaced_deletions),
            (DeletionAction::Import, result.imported_deletions),
        ] {
            for node in nodes {
                if deletions
                    .insert(node.file_id.clone(), (action, node))
                    .is_some()
                {
                    return Err(invalid());
                }
            }
        }
        // read_result already admitted unique retained file identities.
        let retained = result
            .retained_lifecycle
            .into_iter()
            .map(|row| (row.file_id.clone(), row))
            .collect();
        Ok(Self {
            deletions,
            retained,
        })
    }

    pub(crate) fn deletion(&self, file: &str) -> Result<(DeletionAction, &FileDeletionNode)> {
        self.deletions
            .get(file)
            .map(|(action, node)| (*action, node))
            .ok_or_else(invalid)
    }

    pub(crate) fn retained(&self, file: &str) -> Result<&CopiedLifecycle> {
        self.retained.get(file).ok_or_else(invalid)
    }
}
