//! Permanent deletion/undo membership, including nodes no longer current.
//! This is only an index of receipt owners. Publication admission remains the
//! lifecycle verifier's responsibility; malformed ownership is never omitted.
use super::{DerivedOwners, FileRows, State, Writes};
use storage_contract::{OperationId, OperationResult, SharedRecords};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DeletionOwner<'a> {
    Other,
    File(&'a str),
    Malformed,
}
pub(crate) fn owner(receipt: &OperationResult) -> DeletionOwner<'_> {
    let field = match receipt.kind.as_str() {
        "file.delete" => "deletion_node",
        "file.delete.undo" => "removed_deletion_node",
        _ => return DeletionOwner::Other,
    };
    match receipt
        .result
        .get(field)
        .and_then(|node| node.get("file_id"))
        .and_then(json_codec::Value::as_str)
    {
        Some(file) if storage_contract::validate_id(file, "file").is_ok() => {
            DeletionOwner::File(file)
        }
        Some(_) | None => DeletionOwner::Malformed,
    }
}
impl DerivedOwners {
    pub(crate) fn malformed_deletion_receipts(&self) -> bool {
        !self.unscoped_deletions.is_empty()
    }
    fn deletion_member(&mut self, id: OperationId, owner: DeletionOwner<'_>, add: bool) {
        let rows = match owner {
            DeletionOwner::Other => return,
            DeletionOwner::Malformed => &mut self.unscoped_deletions,
            DeletionOwner::File(file) => {
                if add && !self.file_rows.contains_key(file) {
                    self.file_rows.insert(file.to_owned(), FileRows::default());
                }
                &mut self
                    .file_rows
                    .get_mut(file)
                    .expect("invariant: retained lifecycle has its file membership")
                    .deletion_operations
            }
        };
        if add {
            assert!(
                rows.insert(id, ()).is_none(),
                "invariant: deletion receipt contributes one membership"
            );
        } else {
            assert!(
                rows.remove_discard(&id),
                "invariant: old deletion receipt contributes its membership"
            );
            if let DeletionOwner::File(file) = owner
                && self
                    .file_rows
                    .get(file)
                    .expect("invariant: file group exists")
                    .is_empty()
            {
                self.file_rows.remove_discard(file);
            }
        }
    }
    pub(super) fn build_deletion_rows(&mut self, state: &State) {
        for (id, receipt) in state.operations.iter() {
            self.deletion_member(*id, owner(receipt), true);
        }
    }
    pub(super) fn update_deletion_rows(&mut self, before: &State, after: &State, writes: &Writes) {
        for id in &writes.operations {
            let previous = before
                .operations
                .get(id)
                .map_or(DeletionOwner::Other, owner);
            let next = after.operations.get(id).map_or(DeletionOwner::Other, owner);
            if previous != next {
                self.deletion_member(*id, previous, false);
                self.deletion_member(*id, next, true);
            }
        }
    }
}

// A typed operation key avoids stringify/reparse at every file materialization.
pub(crate) type DeletionOperations = SharedRecords<(), OperationId>;
