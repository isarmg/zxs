//! Permanent deletion/undo membership, including nodes no longer current.
//! This is only an index of receipt owners. Publication admission remains the
//! lifecycle verifier's responsibility; malformed ownership is never omitted.
use super::{DerivedOwners, FileRows, State, Writes};
use storage_contract::{OperationId, OperationResult, SharedRecords};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DeletionOwner<'a> {
    Other,
    File(&'a str),
    Files(Vec<&'a str>),
    Malformed,
}
pub(crate) fn owner(receipt: &OperationResult) -> DeletionOwner<'_> {
    if receipt.kind == crate::business::FILE_HISTORY_MOVE_KIND
        && let Some(replacement) = receipt.result.get("replacement")
    {
        let source = receipt
            .result
            .get("deletion_node")
            .and_then(|node| node.get("file_id"))
            .and_then(json_codec::Value::as_str);
        let target = replacement
            .get("deletion_node")
            .and_then(|node| node.get("file_id"))
            .and_then(json_codec::Value::as_str);
        return match (source, target) {
            (Some(source), Some(target))
                if source != target
                    && storage_contract::validate_id(source, "file").is_ok()
                    && storage_contract::validate_id(target, "file").is_ok() =>
            {
                DeletionOwner::Files(vec![source, target])
            }
            _ => DeletionOwner::Malformed,
        };
    }
    if receipt.kind == crate::file_deletion::batch::KIND {
        let Some(nodes) = receipt
            .result
            .get("deletion_nodes")
            .and_then(json_codec::Value::as_array)
        else {
            return DeletionOwner::Malformed;
        };
        if nodes.is_empty() || nodes.len() > crate::file_deletion::batch::MAX_FILES {
            return DeletionOwner::Malformed;
        }
        let mut files = std::collections::BTreeSet::new();
        for node in nodes {
            let Some(file) = node.get("file_id").and_then(json_codec::Value::as_str) else {
                return DeletionOwner::Malformed;
            };
            if storage_contract::validate_id(file, "file").is_err() || !files.insert(file) {
                return DeletionOwner::Malformed;
            }
        }
        return DeletionOwner::Files(files.into_iter().collect());
    }
    let field = match receipt.kind.as_str() {
        "file.delete" | crate::business::FILE_HISTORY_MOVE_KIND => "deletion_node",
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
pub(crate) fn history_copy_owner(receipt: &OperationResult) -> DeletionOwner<'_> {
    if !crate::business::history_copy::is_kind(&receipt.kind)
        && receipt.kind != crate::business::FILE_HISTORY_MOVE_KIND
    {
        return DeletionOwner::Other;
    }
    match receipt
        .result
        .get("file_id")
        .and_then(json_codec::Value::as_str)
    {
        Some(file) if storage_contract::validate_id(file, "file").is_ok() => {
            DeletionOwner::File(file)
        }
        Some(_) | None => DeletionOwner::Malformed,
    }
}
#[derive(Clone, Copy)]
enum Membership {
    Deletion,
    HistoryCopy,
}
impl DerivedOwners {
    pub(crate) fn malformed_deletion_receipts(&self) -> bool {
        !self.unscoped_deletions.is_empty()
    }
    pub(crate) fn malformed_history_copy_receipts(&self) -> bool {
        !self.unscoped_history_copies.is_empty()
    }
    fn receipt_member(
        &mut self,
        id: OperationId,
        owner: &DeletionOwner<'_>,
        membership: Membership,
        add: bool,
    ) {
        let rows = match owner {
            DeletionOwner::Other => return,
            DeletionOwner::Files(files) => {
                for file in files {
                    self.receipt_member(id, &DeletionOwner::File(file), membership, add);
                }
                return;
            }
            DeletionOwner::Malformed => match membership {
                Membership::Deletion => &mut self.unscoped_deletions,
                Membership::HistoryCopy => &mut self.unscoped_history_copies,
            },
            DeletionOwner::File(file) => {
                if add && !self.file_rows.contains_key(*file) {
                    self.file_rows
                        .insert((*file).to_owned(), FileRows::default());
                }
                let rows = self
                    .file_rows
                    .get_mut(*file)
                    .expect("invariant: retained receipt has its file membership");
                match membership {
                    Membership::Deletion => &mut rows.deletion_operations,
                    Membership::HistoryCopy => &mut rows.history_copies,
                }
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
                    .get(*file)
                    .expect("invariant: file group exists")
                    .is_empty()
            {
                self.file_rows.remove_discard(*file);
            }
        }
    }
    pub(super) fn build_deletion_rows(&mut self, state: &State) {
        for (id, receipt) in state.operations.iter() {
            self.receipt_member(*id, &owner(receipt), Membership::Deletion, true);
            self.receipt_member(
                *id,
                &history_copy_owner(receipt),
                Membership::HistoryCopy,
                true,
            );
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
                self.receipt_member(*id, &previous, Membership::Deletion, false);
                self.receipt_member(*id, &next, Membership::Deletion, true);
            }
            let previous = before
                .operations
                .get(id)
                .map_or(DeletionOwner::Other, history_copy_owner);
            let next = after
                .operations
                .get(id)
                .map_or(DeletionOwner::Other, history_copy_owner);
            if previous != next {
                self.receipt_member(*id, &previous, Membership::HistoryCopy, false);
                self.receipt_member(*id, &next, Membership::HistoryCopy, true);
            }
        }
    }
}

// A typed operation key avoids stringify/reparse at every file materialization.
pub(crate) type DeletionOperations = SharedRecords<(), OperationId>;
