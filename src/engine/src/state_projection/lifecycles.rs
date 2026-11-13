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
fn object_members<'a>(receipt: &'a OperationResult, fields: &[&str]) -> DeletionOwner<'a> {
    let mut files = std::collections::BTreeSet::new();
    for field in fields {
        let Some(rows) = receipt
            .result
            .get(*field)
            .and_then(json_codec::Value::as_array)
        else {
            return DeletionOwner::Malformed;
        };
        for row in rows {
            let Some(file) = row.get("file_id").and_then(json_codec::Value::as_str) else {
                return DeletionOwner::Malformed;
            };
            if storage_contract::validate_id(file, "file").is_err() || !files.insert(file) {
                return DeletionOwner::Malformed;
            }
        }
    }
    DeletionOwner::Files(files.into_iter().collect())
}
pub(crate) fn owner(receipt: &OperationResult) -> DeletionOwner<'_> {
    if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
        return object_members(
            receipt,
            &[
                "source_deletions",
                "displaced_deletions",
                "imported_deletions",
            ],
        );
    }
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
    if crate::file_deletion::batch::is_kind(&receipt.kind)
        || crate::file_deletion::directory::is_kind(&receipt.kind)
    {
        let Some(nodes) = receipt
            .result
            .get(
                if matches!(
                    receipt.kind.as_str(),
                    crate::file_deletion::directory::UNDO | crate::file_deletion::batch::UNDO
                ) {
                    "removed_deletion_nodes"
                } else {
                    "deletion_nodes"
                },
            )
            .and_then(json_codec::Value::as_array)
        else {
            return DeletionOwner::Malformed;
        };
        if (nodes.is_empty() && crate::file_deletion::batch::is_kind(&receipt.kind))
            || nodes.len() > crate::file_deletion::batch::MAX_FILES
        {
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
    if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
        return object_members(receipt, &["retained_lifecycle"]);
    }
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
/// Receipt-derived ownership only. Parsing failures remain scoped to every
/// history so the fallible field materializer cannot silently omit bad evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuditOwner {
    Histories(std::collections::BTreeSet<storage_contract::HistoryTreeId>),
    Malformed,
}

fn audit_owner(state: &State, id: OperationId) -> Option<AuditOwner> {
    state.operations.get(&id)?;
    match crate::history_package::path_audit::carrier(state, id) {
        Ok(Some(carrier)) => Some(AuditOwner::Histories(carrier.histories())),
        Ok(None) => None,
        Err(_) => Some(AuditOwner::Malformed),
    }
}

impl DerivedOwners {
    pub(super) fn path_audit_histories(
        &self,
        state: &State,
        key: &str,
    ) -> std::collections::BTreeSet<String> {
        let Ok(id) = key.parse::<OperationId>() else {
            return state.histories.keys().cloned().collect();
        };
        match self.path_audit_owners.get(&id) {
            Some(AuditOwner::Histories(histories)) => {
                histories.iter().map(ToString::to_string).collect()
            }
            Some(AuditOwner::Malformed) => state.histories.keys().cloned().collect(),
            None => std::collections::BTreeSet::new(),
        }
    }

    fn update_audit_owner(&mut self, state: &State, id: OperationId) {
        let next = audit_owner(state, id);
        if self.path_audit_owners.get(&id) == next.as_ref() {
            return;
        }
        self.unscoped_path_audit.remove_discard(&id);
        match next {
            Some(owner) => {
                if owner == AuditOwner::Malformed {
                    self.unscoped_path_audit.insert(id, ());
                }
                self.path_audit_owners.insert(id, owner);
            }
            None => {
                self.path_audit_owners.remove_discard(&id);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Membership {
    Deletion,
    HistoryCopy,
    PathChange,
}

#[derive(PartialEq, Eq)]
pub(crate) enum PathOwner {
    Other,
    Files(std::collections::BTreeSet<storage_contract::FileId>),
    Malformed,
}

pub(crate) fn path_owner(receipt: &OperationResult) -> PathOwner {
    if receipt.kind == crate::history_package::path_audit::RESTORE_KIND {
        return match crate::history_package::path_audit::restored(receipt) {
            Ok(sources) => PathOwner::Files(
                sources.values()
                    .flat_map(|source| source.projections.iter().map(|row| row.file_id))
                    .collect(),
            ),
            Err(_) => PathOwner::Malformed,
        };
    }
    if !crate::persist::path_changes::is_kind(&receipt.kind) {
        return PathOwner::Other;
    }
    match crate::persist::path_changes::read(receipt) {
        Ok(facts) => PathOwner::Files(facts.into_iter().map(|fact| fact.file_id).collect()),
        Err(_) => PathOwner::Malformed,
    }
}

impl DerivedOwners {
    pub(crate) fn malformed_path_operations(&self) -> impl Iterator<Item = &OperationId> {
        self.unscoped_path_changes.keys()
    }

    fn path_member(&mut self, id: OperationId, owner: &PathOwner, add: bool) {
        match owner {
            PathOwner::Other => {}
            PathOwner::Malformed => {
                self.receipt_member(id, &DeletionOwner::Malformed, Membership::PathChange, add);
            }
            PathOwner::Files(files) => {
                for file in files {
                    self.receipt_member(
                        id,
                        &DeletionOwner::File(&file.to_string()),
                        Membership::PathChange,
                        add,
                    );
                }
            }
        }
    }
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
                Membership::PathChange => &mut self.unscoped_path_changes,
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
                    Membership::PathChange => &mut rows.path_operations,
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
            self.update_audit_owner(state, *id);
            self.path_member(*id, &path_owner(receipt), true);
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
            self.update_audit_owner(after, *id);
            let previous_paths = before.operations.get(id).map_or(PathOwner::Other, path_owner);
            let next_paths = after.operations.get(id).map_or(PathOwner::Other, path_owner);
            if previous_paths != next_paths {
                self.path_member(*id, &previous_paths, false);
                self.path_member(*id, &next_paths, true);
            }
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

/// Only lifecycle membership for one file, not a general State projection.
/// All receipts are classified so malformed unrelated ownership still fails closed.
pub(crate) struct LifecycleSource<'a> {
    state: &'a State,
    file: &'a storage_contract::history_window::HistoryFileId,
    deletions: DeletionOperations,
    copies: DeletionOperations,
    malformed: bool,
    moves: crate::state_projection::move_receipts::MoveReceipts<'a>,
}

impl<'a> LifecycleSource<'a> {
    pub(crate) fn capture(
        state: &'a State,
        file: &'a storage_contract::history_window::HistoryFileId,
        mut checkpoint: impl FnMut() -> crate::Result<()>,
    ) -> crate::Result<Self> {
        checkpoint()?;
        let mut source = Self {
            state,
            file,
            deletions: DeletionOperations::new(),
            copies: DeletionOperations::new(),
            malformed: false,
            moves: crate::state_projection::move_receipts::MoveReceipts::new(state),
        };
        for (id, receipt) in state.operations.iter() {
            checkpoint()?;
            for (members, owner) in [
                (&mut source.deletions, owner(receipt)),
                (&mut source.copies, history_copy_owner(receipt)),
            ] {
                let included = match owner {
                    DeletionOwner::Other => false,
                    DeletionOwner::Malformed => {
                        source.malformed = true;
                        false
                    }
                    DeletionOwner::File(owner) => owner == file.as_str(),
                    DeletionOwner::Files(owners) => owners.contains(&file.as_str()),
                };
                if included {
                    members.insert(*id, ());
                }
            }
        }
        checkpoint()?;
        Ok(source)
    }

    pub(crate) fn view(&self) -> LifecycleView<'_> {
        LifecycleView {
            state: self.state,
            file: self.file,
            members: Some(LifecycleMembers {
                deletions: &self.deletions,
                copies: &self.copies,
            }),
            malformed: self.malformed,
            moves: &self.moves,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LifecycleView<'a> {
    state: &'a State,
    file: &'a storage_contract::history_window::HistoryFileId,
    members: Option<LifecycleMembers<'a>>,
    malformed: bool,
    moves: &'a crate::state_projection::move_receipts::MoveReceipts<'a>,
}

#[derive(Clone, Copy)]
struct LifecycleMembers<'a> {
    deletions: &'a DeletionOperations,
    copies: &'a DeletionOperations,
}

impl<'a> LifecycleView<'a> {
    pub(crate) fn state(self) -> &'a State {
        self.state
    }

    pub(crate) fn file(self) -> &'a storage_contract::history_window::HistoryFileId {
        self.file
    }

    pub(crate) fn deletions(self) -> Option<&'a DeletionOperations> {
        self.members.map(|members| members.deletions)
    }

    pub(crate) fn copies(self) -> Option<&'a DeletionOperations> {
        self.members.map(|members| members.copies)
    }

    pub(crate) fn malformed(self) -> bool {
        self.malformed
    }

    pub(crate) fn object_move(
        self,
        id: OperationId,
    ) -> crate::Result<std::sync::Arc<crate::state_projection::move_receipts::MoveReceipt>> {
        self.moves.get(id)
    }
}

impl<'a> crate::state_projection::StateProjection<'a> {
    pub(crate) fn lifecycle(
        self,
        file: &'a storage_contract::history_window::HistoryFileId,
    ) -> LifecycleView<'a> {
        let rows = self.owners.file_rows(file.as_str());
        LifecycleView {
            state: self.state,
            file,
            members: rows.map(|rows| LifecycleMembers {
                deletions: &rows.deletion_operations,
                copies: &rows.history_copies,
            }),
            malformed: self.owners.malformed_deletion_receipts()
                || self.owners.malformed_history_copy_receipts(),
            moves: self.moves,
        }
    }
}
